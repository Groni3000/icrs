//! Reading many values in as few calls to the game as possible.
//!
//! Measuring (`icrs bench`) said something useful: under Proton a 4-byte
//! `ReadProcessMemory` and a 4096-byte one both cost ~18us, because the price
//! is the round trip to the wineserver, not the bytes. A full sample read one
//! value at a time cost ~660 of those round trips - 49 values, each walking
//! its own 7-to-12 hop pointer chain from the module base - and so took 12ms.
//!
//! Two things fix that, and both are in here:
//!
//! * **Walk each address once.** Chains overlap heavily; nearly everything
//!   hangs off the same `Controller.userData`. Every dereference is memoised
//!   for the duration of one read, so a shared prefix costs one call no matter
//!   how many values hang off it.
//! * **Read each object once.** Values in the same object are fetched as one
//!   block and sliced locally, so twenty fields of one object cost one call
//!   instead of twenty.
//!
//! The walk is deliberately redone from the module base on every sample
//! rather than caching final addresses between ticks: a zone change or a
//! formation change swaps the objects underneath, and a stale address reads
//! plausible nonsense instead of failing.

use std::collections::HashMap;
use std::io;

use crate::catalog::Chain;
use crate::offsets::Kind;
use crate::value::Value;

/// How much of one object a single block read may cover. Fields further out
/// than this are read on their own.
const BLOCK_MAX: usize = 4096;

/// What a batch needs from the game. A trait so the batching itself can be
/// tested against a fake address space, with no game and no Windows.
pub trait Reader {
    /// The pointer stored at `address`.
    fn pointer(&self, address: u64) -> io::Result<u64>;

    /// Fill `buf` from `address`; a short read is an error.
    fn block(&self, address: u64, buf: &mut [u8]) -> io::Result<()>;

    /// The Mono string whose pointer is stored at `address`.
    fn text(&self, address: u64) -> io::Result<String>;
}

#[cfg(windows)]
impl Reader for crate::mem::Process {
    fn pointer(&self, address: u64) -> io::Result<u64> {
        self.read_u64(address)
    }

    fn block(&self, address: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read_into(address, buf)
    }

    fn text(&self, address: u64) -> io::Result<String> {
        self.read_mono_string(address)
    }
}

/// A set of values to read together.
///
/// Slots are handed out in order and are how a caller gets its values back:
/// `push` returns one, and `read` returns a vector indexed by it.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    base: u64,
    offsets: Vec<u64>,
    kind: Kind,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add one value, by the same base-and-offsets walk `Process::resolve`
    /// does. Returns its slot.
    pub fn push(&mut self, base: u64, offsets: &[u64], kind: Kind) -> usize {
        self.entries.push(Entry { base, offsets: offsets.to_vec(), kind });
        self.entries.len() - 1
    }

    /// Add one resolved chain. Returns its slot.
    pub fn push_chain(&mut self, module: u64, chain: &Chain) -> usize {
        self.push(chain.base(module), &chain.offsets, chain.kind)
    }
}

/// One pass over the game's memory: the dereferences it has already done, and
/// how many calls that took.
pub struct Walk<'a, R: Reader + ?Sized> {
    reader: &'a R,
    seen: HashMap<u64, u64>,
    pointers: usize,
    blocks: usize,
}

impl<'a, R: Reader + ?Sized> Walk<'a, R> {
    pub fn new(reader: &'a R) -> Self {
        Self { reader, seen: HashMap::new(), pointers: 0, blocks: 0 }
    }

    /// Calls made to the game so far. The number this whole module exists to
    /// keep small, so it is worth being able to assert on.
    pub fn calls(&self) -> usize {
        self.pointers + self.blocks
    }

    /// Of those calls, the ones spent following pointers rather than reading
    /// values. It says which half is worth attacking next.
    pub fn pointers(&self) -> usize {
        self.pointers
    }

    pub fn blocks(&self) -> usize {
        self.blocks
    }

    /// Dereference an address, at most once per pass.
    fn pointer(&mut self, address: u64) -> io::Result<u64> {
        if let Some(value) = self.seen.get(&address) {
            return Ok(*value);
        }
        self.pointers += 1;
        let value = self.reader.pointer(address)?;
        if value == 0 {
            return Err(io::Error::other(format!("null pointer at {address:#x}")));
        }
        self.seen.insert(address, value);
        Ok(value)
    }

    /// Read every value in a batch, grouping the fields that turn out to live
    /// in the same object. A value that cannot be read is `None`; the rest are
    /// unaffected.
    pub fn values(&mut self, batch: &Batch) -> Vec<Option<Value>> {
        let mut values: Vec<Option<Value>> = vec![None; batch.entries.len()];

        // Where each value ended up: the object holding it, and the offset
        // into that object. Objects are what the block reads are grouped by.
        let mut groups: Vec<(u64, Vec<Field>)> = Vec::new();
        let mut texts: Vec<(usize, u64)> = Vec::new();

        for (slot, entry) in batch.entries.iter().enumerate() {
            let Some((object, offset)) = self.locate(entry) else {
                continue;
            };
            if entry.kind == Kind::Utf16 {
                // A string field is only a pointer; following it is its own
                // pair of reads, so it is not worth a block.
                texts.push((slot, object + offset));
                continue;
            }
            let field = Field { slot, offset, kind: entry.kind };
            match groups.iter_mut().find(|(at, _)| *at == object) {
                Some((_, fields)) => fields.push(field),
                None => groups.push((object, vec![field])),
            }
        }

        for (object, fields) in &groups {
            self.read_group(*object, fields, &mut values);
        }
        for (slot, address) in texts {
            // Three calls really: the pointer, the length and the characters.
            self.blocks += 3;
            if let Ok(text) = self.reader.text(address) {
                values[slot] = Some(Value::Text(text));
            }
        }

        values
    }

    /// Walk a chain to the object holding its value, and the offset within it.
    fn locate(&mut self, entry: &Entry) -> Option<(u64, u64)> {
        let (&last, prefix) = entry.offsets.split_last()?;
        let mut address = entry.base;
        for &offset in prefix {
            address = self.pointer(address).ok()? + offset;
        }
        Some((self.pointer(address).ok()?, last))
    }

    /// One call for the whole object when that works, and one call per field
    /// when it does not - an object can sit at the end of its pages, and a
    /// block that runs past them fails as a whole.
    fn read_group(&mut self, object: u64, fields: &[Field], values: &mut [Option<Value>]) {
        let span = fields
            .iter()
            .map(|field| field.offset as usize + field.kind.size())
            .max()
            .unwrap_or(0);

        if fields.len() > 1 && span <= BLOCK_MAX {
            let mut block = vec![0u8; span];
            self.blocks += 1;
            if self.reader.block(object, &mut block).is_ok() {
                for field in fields {
                    values[field.slot] = Value::decode(field.kind, &block[field.offset as usize..]);
                }
                return;
            }
        }

        for field in fields {
            let mut bytes = vec![0u8; field.kind.size()];
            self.blocks += 1;
            if self.reader.block(object + field.offset, &mut bytes).is_ok() {
                values[field.slot] = Value::decode(field.kind, &bytes);
            }
        }
    }
}

struct Field {
    slot: usize,
    offset: u64,
    kind: Kind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A toy address space: bytes that exist, and nothing else readable.
    #[derive(Default)]
    struct Fake {
        bytes: HashMap<u64, u8>,
        calls: Cell<usize>,
        /// Refuse reads larger than this, to stand in for an object sitting at
        /// the end of its pages.
        max_read: Option<usize>,
    }

    impl Fake {
        fn put(&mut self, address: u64, bytes: &[u8]) {
            for (step, byte) in bytes.iter().enumerate() {
                self.bytes.insert(address + step as u64, *byte);
            }
        }

        /// Map a region as zeroes, the way a real object's bytes all exist.
        fn fill(&mut self, address: u64, length: u64) {
            for step in 0..length {
                self.bytes.insert(address + step, 0);
            }
        }

        fn put_u64(&mut self, address: u64, value: u64) {
            self.put(address, &value.to_le_bytes());
        }

        fn put_i32(&mut self, address: u64, value: i32) {
            self.put(address, &value.to_le_bytes());
        }

        fn put_string(&mut self, object: u64, text: &str) {
            let wide: Vec<u16> = text.encode_utf16().collect();
            self.put_i32(object + 0x10, wide.len() as i32);
            for (step, unit) in wide.iter().enumerate() {
                self.put(object + 0x14 + step as u64 * 2, &unit.to_le_bytes());
            }
        }
    }

    impl Reader for Fake {
        fn pointer(&self, address: u64) -> io::Result<u64> {
            let mut bytes = [0u8; 8];
            self.block(address, &mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        }

        fn block(&self, address: u64, buf: &mut [u8]) -> io::Result<()> {
            self.calls.set(self.calls.get() + 1);
            if self.max_read.is_some_and(|most| buf.len() > most) {
                return Err(io::Error::other("block too large for the mapping"));
            }
            for (step, slot) in buf.iter_mut().enumerate() {
                match self.bytes.get(&(address + step as u64)) {
                    Some(byte) => *slot = *byte,
                    None => return Err(io::Error::other(format!("unmapped at {address:#x}"))),
                }
            }
            Ok(())
        }

        fn text(&self, address: u64) -> io::Result<String> {
            let object = self.pointer(address)?;
            let mut length = [0u8; 4];
            self.block(object + 0x10, &mut length)?;
            let units = i32::from_le_bytes(length) as usize;
            let mut bytes = vec![0u8; units * 2];
            self.block(object + 0x14, &mut bytes)?;
            let wide: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            Ok(String::from_utf16_lossy(&wide))
        }
    }

    const BASE: u64 = 0x1000;
    const OBJECT: u64 = 0x5000;
    const DEEPER: u64 = 0x9000;

    /// base -> OBJECT, and OBJECT+0x40 -> DEEPER. Both objects are mapped
    /// as whole blocks, like real ones: a block read only works because the
    /// bytes between the fields exist too.
    fn memory() -> Fake {
        let mut fake = Fake::default();
        fake.fill(BASE, 0x8);
        fake.fill(OBJECT, 0x100);
        fake.fill(DEEPER, 0x100);
        fake.put_u64(BASE, OBJECT);
        fake.put_i32(OBJECT + 0x10, 270);
        fake.put_i32(OBJECT + 0x14, 86);
        fake.put(OBJECT + 0x18, &[1]);
        fake.put(OBJECT + 0x20, &4.5f64.to_le_bytes());
        fake.put(OBJECT + 0x28, &1158i64.to_le_bytes());
        fake.put(OBJECT + 0x30, &7i64.to_le_bytes()); // quad mantissa
        fake.put(OBJECT + 0x38, &3i64.to_le_bytes()); // quad exponent
        fake.put_u64(OBJECT + 0x40, DEEPER);
        fake.put_i32(DEEPER + 0x8, 42);
        fake
    }

    #[test]
    fn many_fields_of_one_object_cost_one_block() {
        let fake = memory();
        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let highest = batch.push(BASE, &[0x14], Kind::I32);
        let flag = batch.push(BASE, &[0x18], Kind::U8);
        let since = batch.push(BASE, &[0x20], Kind::F64);
        let stacks = batch.push(BASE, &[0x28], Kind::I64);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);

        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(values[highest], Some(Value::Int(86)));
        assert_eq!(values[flag], Some(Value::Int(1)));
        assert_eq!(values[since], Some(Value::Float(4.5)));
        assert_eq!(values[stacks], Some(Value::Int(1158)));
        // One dereference of the base, one block for the object. Reading them
        // one at a time would have been ten.
        assert_eq!(walk.calls(), 2);
    }

    #[test]
    fn a_shared_prefix_is_dereferenced_once() {
        let fake = memory();
        let mut batch = Batch::new();
        // Both of these walk base -> OBJECT -> DEEPER; only the last offset
        // differs, and the second value is in the deeper object.
        let near = batch.push(BASE, &[0x10], Kind::I32);
        let far = batch.push(BASE, &[0x40, 0x8], Kind::I32);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[near], Some(Value::Int(270)));
        assert_eq!(values[far], Some(Value::Int(42)));
        // base dereferenced once (shared), OBJECT+0x40 dereferenced once,
        // then one read per object.
        assert_eq!(walk.calls(), 4);
    }

    #[test]
    fn a_quad_crosses_two_words_inside_the_block() {
        let fake = memory();
        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let quad = batch.push(BASE, &[0x30], Kind::Quad);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(
            values[quad],
            Some(Value::Big(crate::mem::Quad { mantissa: 7, exponent: 3 }))
        );
        assert_eq!(walk.calls(), 2);
    }

    #[test]
    fn a_block_that_cannot_be_read_falls_back_to_single_fields() {
        // An object at the end of its pages: the wide read fails, every field
        // still arrives.
        let mut fake = memory();
        fake.max_read = Some(8);

        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let highest = batch.push(BASE, &[0x14], Kind::I32);
        let stacks = batch.push(BASE, &[0x28], Kind::I64);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(values[highest], Some(Value::Int(86)));
        assert_eq!(values[stacks], Some(Value::Int(1158)));
        // The failed block, then one read each.
        assert_eq!(walk.calls(), 1 + 1 + 3);
    }

    #[test]
    fn one_unreadable_field_does_not_take_the_others_with_it() {
        let fake = memory();
        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let nowhere = batch.push(BASE, &[0x800], Kind::I32); // past the mapping
        let highest = batch.push(BASE, &[0x14], Kind::I32);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(values[highest], Some(Value::Int(86)));
        assert_eq!(values[nowhere], None);
    }

    #[test]
    fn a_null_in_the_walk_is_a_miss_not_a_panic() {
        let mut fake = memory();
        fake.put_u64(OBJECT + 0x40, 0);

        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let dead = batch.push(BASE, &[0x40, 0x8], Kind::I32);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)), "the live value still arrives");
        assert_eq!(values[dead], None);
    }

    #[test]
    fn an_unmapped_base_loses_only_its_own_values() {
        let fake = memory();
        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let orphan = batch.push(0xDEAD_0000, &[0x10], Kind::I32);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(values[orphan], None);
    }

    #[test]
    fn strings_are_followed_separately() {
        let mut fake = memory();
        fake.fill(0xB000, 0x40);
        fake.put_u64(OBJECT + 0x48, 0xB000);
        fake.put_string(0xB000, "Ellywick");

        let mut batch = Batch::new();
        let zone = batch.push(BASE, &[0x10], Kind::I32);
        let name = batch.push(BASE, &[0x48], Kind::Utf16);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[zone], Some(Value::Int(270)));
        assert_eq!(values[name], Some(Value::Text("Ellywick".to_string())));
        assert_eq!(walk.pointers(), 1, "the base, shared with the scalar");
    }

    #[test]
    fn an_empty_batch_touches_nothing() {
        let fake = memory();
        let mut walk = Walk::new(&fake);
        assert!(walk.values(&Batch::new()).is_empty());
        assert_eq!(walk.calls(), 0);
    }

    #[test]
    fn a_far_field_is_read_on_its_own() {
        // Beyond BLOCK_MAX from the object, so it must not drag a 64KB read
        // in behind it.
        let mut fake = memory();
        fake.fill(OBJECT + 0x2000, 0x10);
        fake.put_i32(OBJECT + 0x2000, 99);

        let mut batch = Batch::new();
        let near = batch.push(BASE, &[0x10], Kind::I32);
        let far = batch.push(BASE, &[0x2000], Kind::I32);

        let mut walk = Walk::new(&fake);
        let values = walk.values(&batch);
        assert_eq!(values[near], Some(Value::Int(270)));
        assert_eq!(values[far], Some(Value::Int(99)));
        // The oversized span sends the group down the per-field path.
        assert_eq!(walk.calls(), 1 + 2);
    }
}
