// Copyright 2024 by Michał Nazarewicz <mina86@mina86.com>
//
// This code is free software: you can redistribute it and/or modify it under
// the terms of the GNU Lesser General Public License as published by the Free
// Software Foundation; either version 2 of the License, or (at your option) any
// later version.
//
// This code is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR
// A PARTICULAR PURPOSE.  See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// srgb crate.  If not, see <http://www.gnu.org/licenses/>.

use solana_program::pubkey::Pubkey;

/// Extracts heap size from the data passed to contract’s entrypoint.
///
/// Sender of a Solana transaction can request heap size allocated for smart
/// contracts to be increased over the default 32 KiB size.  To accomplish that,
/// they send an instruction calling Compute Budget program.
///
/// This function scans the data passed to the entrypoint in search for such
/// instructions.  If they exist, it returns requested heap size.
///
/// This function performs no allocations and is intended to be used as the
/// first thing in smart contract’s entrypoint to get the heap size and properly
/// initialise the allocator.
///
/// # Safety
///
/// `input` must be a correctly serialised instruction input data as serialised
/// by Solana runtime.  This includes format of the encoding as well as
/// alignment of the data.
///
/// This condition is upheld by the Solana runtime so it’s safe to call this
/// function inside of smart contract’s the `entrypoint` function.
pub unsafe fn extract_heap_size(
    input: *const u8,
) -> Option<u64> {
    // SAFETY: Caller promises to uphold all necessary conditions.
    let ix_sysvar_data = unsafe { AccountsInfoIter::new(input) }
        .filter_map(|it| it)
        .filter(|(id, _)| solana_program::sysvar::instructions::check_id(id) )
        .map(|(_, data)| data)
        .next()?;

    InstructionsIter::new(ix_sysvar_data)
        .take_while(|(id, _data)| compute_budget::check_id(id))
        .filter_map(|(_id, data)| parse_compute_budget_instruction(data))
        .next()
}

/// Iterator over AccountInfo present in the instruction input passed to the
/// smart contract’s entrypoint.
///
/// Each AccountInfo is returned as either `None` if it’s a duplicate of another
/// account or `Some(key, data)`.  Other information about the account (such as
/// owner, lamports etc) are ignored.
struct AccountsInfoIter<'a> {
    cursor: RawCursor,
    count: usize,
    _phantom: core::marker::PhantomData<&'a [u8]>,
}

impl<'a> AccountsInfoIter<'a> {
    /// Constructs a new iterator.
    ///
    /// # Safety
    ///
    /// See [`extract_heap_size`].
    unsafe fn new(input: *const u8) -> Self {
        let mut cursor = RawCursor(input);
        let count = *cursor.get::<u64>() as usize;
        Self { cursor, count, _phantom: core::marker::PhantomData }
    }
}

/// AccountInfo descriptor header.
#[repr(C, packed)]
struct AccountInfoHead {
    /// Indicates whether this entry is a duplicate marker and if so index of
    /// the duplicated account.
    ///
    /// If this equals [`solana_program::entrypoint::NON_DUP_MARKER`] than the
    /// entry is not a duplicate and reading of the account info should proceed.
    ///
    /// Otherwise, the entry is a duplicate and this field indicates index of
    /// the duplicate account.  In that case other fields in this struct should
    /// be ignored and reading of remaining information about the account should
    /// be skipped as well.
    dup_info: u8,

    is_signer: u8,
    is_writable: u8,
    executable: u8,
    original_data_len: u32,
}

impl<'a> core::iter::Iterator for AccountsInfoIter<'a> {
    type Item = Option<(&'a Pubkey, &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        self.count = self.count.checked_sub(1)?;
        // SAFETY: Caller that constructed `self` promises that the data passed
        // during construction is correct.
        //
        // Since, cursor is aligned to eight bytes and all reads are in multiple
        // of eight bytes, cursor is always properly aligned for all the reads.
        // The only read which isn’t in multiple of eight bytes is that of
        // `data` but that is followed by explicit aligning of the cursor.
        Some(Some(unsafe {
            let head = self.cursor.get::<AccountInfoHead>();
            if head.dup_info == solana_program::entrypoint::NON_DUP_MARKER {
                return Some(None);
            }

            let key = self.cursor.get::<Pubkey>();
            let _owner = self.cursor.get::<Pubkey>();
            let _lamports = self.cursor.get::<u64>();

            let data = self.cursor.get_slice();
            self.cursor.get_raw(
                solana_program::entrypoint::MAX_PERMITTED_DATA_INCREASE);
            self.cursor.align(solana_program::entrypoint::BPF_ALIGN_OF_U128);
            let _rent_epoch = self.cursor.get::<u64>();

            (key, data)
        }))
    }
}

/// Iterator over instructions present in Iterations sysvar account’s data.
///
/// Each instruction is returned as `(program_id, data)` pair.  Other
/// information about the instruction (namely accounts metadata) are ignored.
///
/// Note: This isn’t a fused iterator.  If an instruction fails to parse, `next`
/// call will return `None` however subsequent calls to `next` may return
/// correct result.  There is no way to know when the iterator actually stopped.
/// The intention is that if instruction fails to parse caller should stop
/// iteration so this detail is only relevant if caller would assume `next`
/// cannot return `Some` after returning `None`.
struct InstructionsIter<'a> {
    data: &'a [u8],
    offsets: &'a [[u8; 2]],
}

impl<'a> InstructionsIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        let offsets = (|| {
            // Structure of the account data is:
            //
            //     instructions_count: u16,
            //     data_offsets:       [u16; instructions_count],
            //     _rest:              [u8],
            //
            // The data_offsets is array which has instructions_count entries.
            // One for each instruction in the transaction.  Each entry is an
            // offset starting from the start of the account data to start of
            // the data for given instruction.
            let (count, tail) = stdx::split_at::<2, _>(data)?;
            let count = u16::from_ne_bytes(*count) as usize;
            stdx::as_chunks::<2, u8>(tail).0.get(..count)
        })().unwrap_or(&[][..]);
        Self { data, offsets }
    }

    /// Parses instruction read from Instructions sysvar account.
    ///
    /// This is an internal method parsing encoded instruction.
    fn parse_instruction(data: &[u8]) -> Option<(&Pubkey, &[u8])> {
        // Structure of the instruction data is:
        //
        //     accounts_count: u16,
        //     accounts:       [(u8, Pubkey); accounts_count],
        //     program_id:     Pubkey,
        //     data_len:       u16,
        //     data:           [u8; data_len],
        //     _rest:          [u8],
        //
        // Usually caller doesn’t know the length of the instruction and `data`
        // argument contains more data than just the given instruction.

        // Skip over all accounts.
        let (count, data) = stdx::split_at::<2, _>(data)?;
        let count = u16::from_ne_bytes(*count) as usize;
        let offset = count * core::mem::size_of::<(u8, Pubkey)>();
        let data = data.get(offset..)?;

        // Get program id and data
        let (head, data) = stdx::split_at::<34, _>(data)?;
        let (program_id, length) = stdx::split_array_ref::<32, 2, 34>(head);

        // SAFETY: Pubkey is repr(transparent) over [u8; 32] so transmuting is
        // sound.
        let program_id = unsafe { &*crate::ptr::from_ref(program_id).cast() };
        let length = u16::from_ne_bytes(*length) as usize;

        Some((program_id, data.get(..length)?))
    }
}

impl<'a> core::iter::Iterator for InstructionsIter<'a> {
    type Item = (&'a Pubkey, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let (car, cdr) = self.offsets.split_first()?;
        self.offsets = cdr;
        let offsset = u16::from_ne_bytes(*car) as usize;
        Self::parse_instruction(self.data.get(offsset..)?)
    }
}

/// Parses `ComputeBudgetInstruction::RequestHeapFrame` variant.
///
/// Assumes `data` is borsh-serialised `ComputeBudgetInstruction` enum.
/// Deserialises the data and checks whether it’s the `RequestHeapFrame`
/// variant.  If it is, returns the requested heap size.  Otherwise (on parsing
/// failure or if the value holds a different variant) returns `None`.
fn parse_compute_budget_instruction(data: &[u8]) -> Option<u64> {
    // We cannot use borsh since it may perform allocations.  Fortunately,
    // ComputeBudgetInstruction enum is trivial to parse.  Each enum variant is
    // tagged with one-byte discriminant followed by serialisation of the
    // fields.  Integers are serialised as little-endian with no fancy encoding
    // (so u32 is four bytes and u64 is eight).  For definition of the enum see
    // https://docs.rs/solana-sdk/latest/solana_sdk/compute_budget/enum.ComputeBudgetInstruction.html
    data.split_first()
        .filter(|(tag, _rest)| **tag == 1)
        .and_then(|(_tag, data)| data.try_into().ok())
        .map(|bytes| u64::from(u32::from_le_bytes(n)) * 1024)
}

/// Helper providing method for reading data from a raw buffer.
///
/// All operations on this cursor are unsafe and it’s on the caller to guarantee
/// that the cursor points at data valid for each request.
struct RawCursor(*const u8);

impl RawCursor {
    /// Returns pointer to the next `len` bytes and advances the cursor.
    ///
    /// # Safety
    ///
    /// There must be `len` bytes remaining in the buffer this cursor points at.
    unsafe fn get_raw(&mut self, len: usize) -> *const u8 {
        let ptr = self.0;
        self.0 = ptr.add(len);
        ptr
    }

    /// Returns reference to `T` stored at the cursor and advances the cursor to
    /// point just past the object.
    ///
    /// # Safety
    ///
    /// There must be enough space to hold `T`, current cursor’s position must
    /// be properly aligned for `T` and the bytes must hold valid `T`
    /// representation.
    unsafe fn get<'a, T>(&mut self) -> &'a T {
        &*self.get_raw(core::mem::size_of::<T>()).cast()
    }

    /// Reads 8-byte length and then returns slice with that length.  Advances
    /// the cursor to point past the slice.
    ///
    /// # Safety
    ///
    /// Cursor must be properly aligned to read `u64` and the buffer must have
    /// enough space to hold the `u64` and then slice with length read from the
    /// the `u64`.
    unsafe fn get_slice<'a>(&mut self) -> &'a [u8] {
        let len = *self.get::<u64>() as usize;
        let ptr = self.get_raw(len);
        core::slice::from_raw_parts(ptr, len)
    }

    /// Aligns cursor to given alignment advancing it if necessary.
    ///
    /// # Safety
    ///
    /// If cursor isn’t aligned, there must be enough remaining bytes in the
    /// buffer to advance the cursor.
    unsafe fn align(&mut self, alignment: usize) {
        self.0 = self.0.add(self.0.align_offset(alignment));
    }
}

/// Declaration of Compute Budget sysvar program’s ID.
mod compute_budget {
    solana_program::declare_id!("ComputeBudget111111111111111111111111111111");
}
