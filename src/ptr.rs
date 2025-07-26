//! Helper functions for pointer arithmetic.

/// Creates a new pointer with the given address.
// TODO(mina86): Use ptr.with_addr once strict_provenance stabilises.
pub(super) fn with_addr(ptr: *mut u8, addr: usize) -> *mut u8 {
    ptr.wrapping_add(addr.wrapping_sub(ptr as usize))
}

/// Aligns pointer to given alignment which must be a power of two.
///
/// If `align` isn’t a power of two, result is unspecified.
pub(super) fn align(ptr: *mut u8, align: usize) -> *mut u8 {
    let mask = align - 1;
    debug_assert!(align != 0 && align & mask == 0);
    // TODO(mina86): Use ptr.map_addr once strict_provenance stabilises.
    with_addr(ptr, ptr.wrapping_add(mask) as usize & !mask)
}

/// Returns end address of given object.
pub(super) fn end_addr_of_val<T: Sized>(obj: &T) -> usize {
    (obj as *const T).wrapping_add(1) as usize
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
pub(super) fn assert_no_overlap(
    a: *const u8,
    a_size: usize,
    b: *const u8,
    b_size: usize,
) {
    let a = a..a.wrapping_add(a_size);
    let b = b..b.wrapping_add(b_size);
    assert!(
        !a.contains(&b.start) && !a.contains(&b.end),
        "{a:?} and {b:?} overlap",
    )
}
