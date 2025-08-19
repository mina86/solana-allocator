// Rust doesn’t recognise ‘solana’ as a target_os unless building via cargo
// build-sbf.  Silence the warning.
#![cfg(any(test, target_os = "solana"))]
#![cfg_attr(not(target_os = "solana"), allow(unexpected_cfgs))]
#![allow(private_bounds)]

//! Custom global allocator which doesn’t assume 32 KiB heap size.
//!
//! Default Solana allocator assumes there’s only 32 KiB of available heap
//! space.  Since heap size can be changed per-transaction, this assumption is
//! not always accurate.  This module defines a global allocator which doesn’t
//! assume size of available space.

extern crate alloc;

use alloc::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;

#[cfg(test)]
mod tests;

/// Custom bump allocator for on-chain operations.
///
/// The default allocator is also a bump one, but grows from a fixed
/// HEAP_START + 32kb downwards and has no way of making use of extra
/// heap space requested for the transaction.
///
/// This implementation starts at HEAP_START and grows upward, producing
/// a segfault once out of available heap memory.
///
/// In addition, the allocator supports reserving space for global state.  `G`
/// generic argument specifies type of an object which will be allocated at the
/// start of the heap and accessible through [`Self::global`] method.  This is
/// meant to work-around Solana’s lack of support for mutable statics.
pub struct BumpAllocator<G> {
    #[cfg(test)]
    ptr: core::ptr::NonNull<u8>,
    #[cfg(test)]
    layout: Layout,

    _ph: core::marker::PhantomData<G>,
}


/// Start address of the memory region used for program heap.
///
/// This is the same as `solana_sdk::entrypoint::HEAP_START_ADDRESS`.
#[cfg(not(test))]
const HEAP_START_ADDRESS: u64 = 0x3_0000_0000;

/// Minimal length of the heap memory region used for program heap.
///
/// The actual heap size may be larger if Compute Budget Program’s
/// `RequestHeapFrame` instruction was used.
///
/// This is the same as `solana_sdk::entrypoint::HEAP_LENGTH`.
const HEAP_LENGTH: usize = 32 * 1024;


/// Data stored by the [`BumpAllocator`] at the start of the heap.
struct Header<G> {
    /// Amount of used memory or end offset from the end of the header.
    ///
    /// To access the offset, users should call [`Header::get_end_offset`] and
    /// [`Header::set_end_offset`] which operate on offset from the start of
    /// heap.
    used: Cell<u32>,

    /// The global state.
    global: G,
}

impl<G> Header<G> {
    /// Size of the header.
    const SIZE: u32 = match core::mem::size_of::<Header<G>>() {
        size if size <= u32::MAX as usize => size as u32,
        _ => panic!("Header too large"),
    };

    /// Returns end offset from the start of the heap, i.e. offset of the first
    /// byte available for allocation.
    fn get_end_offset(&self) -> u32 { self.used.get() + Self::SIZE }

    /// Sets end offset from the start of the heap (i.e. offset of the first
    /// byte available for allocation) to given value.
    fn set_end_offset(&self, offset: u32) { self.used.set(offset - Self::SIZE) }
}

#[cfg(not(test))]
impl<G> BumpAllocator<G> {
    /// Creates a new global allocator.
    ///
    /// # Safety
    ///
    /// Caller may instantiate only one BumpAllocator and must set it as
    /// a global allocator.
    ///
    /// Using multiple BumpAllocators or using this allocator while other global
    /// allocator is present leads to undefined behaviour since the allocator
    /// needs to take ownership of the heap provided by Solana runtime.
    pub const unsafe fn new() -> Self {
        Self { _ph: core::marker::PhantomData }
    }

    /// Returns start of the heap.
    const fn heap_start(&self) -> *mut u8 { HEAP_START_ADDRESS as *mut u8 }

    /// Returns offset from the start of the heap of given pointer.
    ///
    /// Assumes the pointer falls withing the heap or points one past the end of
    /// the heap.
    fn to_offset(&self, ptr: *mut u8) -> u32 { ptr as usize as u32 }

    /// Returns pointer to a byte at given offset within a heap.
    fn from_offset(&self, offset: u32) -> *mut u8 {
        (u64::from(offset) | HEAP_START_ADDRESS) as *mut u8
    }
}

#[cfg(test)]
impl<G: bytemuck::Zeroable> BumpAllocator<G> {
    /// Creates a new allocator with given amount of available memory.
    ///
    /// `size` is capped at `u32::MAX`.  Panics if allocation fails, or
    /// requested size is less than size of the header.
    fn new(size: usize) -> Self {
        let size = size.min(u32::MAX as usize);
        assert!(size >= core::mem::size_of::<Header<G>>());
        let align = core::mem::align_of::<Header<G>>().max(16);
        let layout = Layout::from_size_align(size, align).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        let ptr = core::ptr::NonNull::new(ptr).unwrap();
        Self { ptr, layout, _ph: core::marker::PhantomData }
    }

    /// Returns amount of used memory in bytes excluding space used for end
    /// position address stored at the start of the heap.
    fn used(&self) -> usize { self.header().used.get() as usize }

    fn heap_start(&self) -> *mut u8 { self.ptr.as_ptr() }
    fn to_offset(&self, ptr: *mut u8) -> u32 {
        (ptr as usize - self.heap_start() as usize) as u32
    }
    fn from_offset(&self, offset: u32) -> *mut u8 {
        self.heap_start().wrapping_add(offset as usize)
    }
}

#[cfg(test)]
impl<G> core::ops::Drop for BumpAllocator<G> {
    fn drop(&mut self) {
        // SAFETY: ptr and layout are the same as when we’ve allocated.
        unsafe { alloc::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}

impl<G: bytemuck::Zeroable> BumpAllocator<G> {
    /// Returns reference to allocator’s internal data stored at the front of
    /// the heap.
    ///
    /// The header includes address of the start of the available free memory
    /// and global state `G` reserved for the users of this allocator.
    fn header(&self) -> &Header<G> {
        // Make sure header does not go past the guaranteed heap space.
        let _: () = const {
            let header_size = core::mem::size_of::<Header<G>>();
            assert!(header_size <= HEAP_LENGTH, "Global state too large")
        };
        // SAFETY:
        // 1. In Solana build, heap is aligned to 2**32 and we’ve just
        //    checked header fits in heap; in test Self::new guarantees
        //    size and alignment hold.
        // 2. The heap has been zero-initialised and Header<G> is Zeroable.
        unsafe { &*self.heap_start().cast() }
    }

    /// Checks whether given slice falls within available heap space and updates
    /// end offset if it does.
    ///
    /// Outside of unit tests, if `poke` Cargo feature is enabled, the check is
    /// done by writing zero byte to the last byte of the slice which will cause
    /// UB if it fails beyond available heap space.
    ///
    /// When run as Solana contract that UB is segfault.  If `poke` Cargo
    /// feature is enabled, the segfault happens when trying to allocate; by
    /// default it’s deferred to the moment region past the heap is accessed by
    /// the client (a bit like over-committing works in Linux).
    ///
    /// If check passes, returns pointer is aligned to `layout.align()`.
    fn update_end_offset(
        &self,
        offset: u32,
        layout: Layout,
    ) -> Option<*mut u8> {
        #[cfg(test)]
        assert!(layout.align() <= self.layout.align());

        let size = u32::try_from(layout.size()).ok()?;
        let mask = (layout.align() - 1) as u32;
        let offset: u32 = offset.checked_add(mask)? & !mask;
        let end_offset = offset.checked_add(size)?;

        #[cfg(test)]
        if end_offset as usize > self.layout.size() {
            return None;
        }
        #[cfg(all(not(test), feature = "poke"))]
        // SAFETY: This is unsound but it will only execute on Solana where
        // accessing memory beyond heap results in segfault which is what we
        // want.
        let _ = unsafe { self.from_offset(end_offset - 1).read_volatile() };

        self.header().set_end_offset(end_offset);
        Some(self.from_offset(offset))
    }

    /// Returns reference to global state `G` reserved on the heap.
    ///
    /// This is meant as a poor man’s mutable statics which are not supported on
    /// Solana.  With it, one may use a `Cell<T>` as global state and access it
    /// from different parts of Solana program.
    ///
    /// Note that by default `G` is a unit type which means that there is no
    /// reserved global state.
    pub fn global(&self) -> &G { &self.header().global }
}

unsafe impl<G: bytemuck::Zeroable> GlobalAlloc for BumpAllocator<G> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.update_end_offset(self.header().get_end_offset(), layout)
            .unwrap_or(core::ptr::null_mut())
    }

    /// Deallocates specified object.
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let header = self.header();
        // If this is the last allocation, free it.  Otherwise this is bump
        // allocator and we leak memory.
        let end_offset = self.to_offset(ptr.wrapping_add(layout.size()));
        if end_offset == header.get_end_offset() {
            header.set_end_offset(self.to_offset(ptr));
        }
    }

    /// Reallocate an object.
    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: Layout,
        new_size: usize,
    ) -> *mut u8 {
        // SAFETY: Caller guarantees new layout is valid.
        let new_layout = unsafe {
            Layout::from_size_align_unchecked(new_size, layout.align())
        };
        let header = self.header();
        let tail = header.get_end_offset();
        if self.to_offset(ptr.wrapping_add(layout.size())) == tail {
            // If this is the last allocation, resize.
            self.update_end_offset(self.to_offset(ptr), new_layout)
                .unwrap_or(core::ptr::null_mut())
        } else if new_size <= layout.size() {
            // If user wants to shrink size, do nothing.  We’re leaking memory
            // here but we’re bump allocator so that’s what we do.
            ptr
        } else if let Some(new_ptr) = self.update_end_offset(tail, new_layout) {
            // Otherwise, we need to make a new allocation and copy.
            // SAFETY: The previously allocated block cannot overlap the
            // newly allocated block.  Note that layout.size() < new_size.
            unsafe { memcpy(new_ptr, ptr, layout.size()) }
            new_ptr
        } else {
            core::ptr::null_mut()
        }
    }
}

/// Copies `size` bytes from `src` to `dst`.
///
/// # Safety
///
/// Caller must guarantees all of the conditions required by
/// [`core::ptr::copy_nonoverlapping`].
pub(super) unsafe fn memcpy(dst: *mut u8, src: *const u8, size: usize) {
    if cfg!(debug_assertions) {
        assert_no_overlap(dst, size, src, size);
    }
    // SAFETY: Caller guarantees all necessary conditions.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, size) }
}

#[track_caller]
fn assert_no_overlap(a: *const u8, a_size: usize, b: *const u8, b_size: usize) {
    let a = a..a.wrapping_add(a_size);
    let b = b..b.wrapping_add(b_size);
    assert!(
        !a.contains(&b.start) && !a.contains(&b.end),
        "{a:?} and {b:?} overlap",
    )
}
