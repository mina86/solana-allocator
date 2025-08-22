#![cfg(target_os = "solana")]

extern crate alloc;

use alloc::alloc::Layout;
use core::fmt;


/// Address of the start of Solana heap.
const HEAP_START_ADDRESS: usize = 0x3_0000_0000;

/// Address of the end of the Solana heap.  The smart contract is run with heap
/// increased from the default 32K.
const HEAP_END_ADDRESS: usize = HEAP_START_ADDRESS + 256 * 1024;


/// Emits a Solana log message without performing memory allocations.  All
/// arguments are converted into Arg objects.
macro_rules! log {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        do_log(format_args!($fmt $(, Arg::from($arg))*));
    };
}
/// Checks that the condition is true.  If it’s not logs.  Returns the
/// condition.
macro_rules! check {
    ($cond:expr, $($args:expr),* $(,)?) => {
        if $cond {
            true
        } else {
            log!($($args),*);
            false
        }
    };
}


/// Solana program’s entrypoint.
///
/// We’re ignoring instruction data and accounts thus it goes straight to
/// running the benchmarks.
///
/// If `free` Cargo feature is enabled, benchmarks allocation and deallocations;
/// otherwise benchmarks allocations only.  Note that in the former case, added
/// overhead of managing the pointers to allocated memory adds instruction to
/// the allocation loop so the allocation benchmark ends up tiny bit slower.
#[no_mangle]
pub unsafe extern "C" fn entrypoint(_input: *mut u8) -> u64 {
    let mut ok = true;
    let mut id = 0;

    macro_rules! do_alloc {
        ($size:expr, $align:expr) => {{
            const LAYOUT: Layout = match Layout::from_size_align($size, $align)
            {
                Ok(x) => x,
                Err(_) => panic!(),
            };
            id += 1;
            let reg = check_alloc(id, LAYOUT);
            ok = ok && reg.is_some();
            reg
        }};
    }

    do_alloc!(1, 1);
    do_alloc!(8, 2);
    let mut a = do_alloc!(9, 1);
    let mut b = do_alloc!(8, 4);
    do_alloc!(99, 1);
    do_alloc!(8, 8);
    do_alloc!(999, 1);
    do_alloc!(8, 16);
    let c = do_alloc!(9999, 1);
    let d = do_alloc!(8, 32);
    let e = do_alloc!(8, 8);

    ok = check_global() && ok;

    // Freeing last allocation always works and redoing the same allocation
    // should return the same region.
    if let Some((c, (d, e))) = c.zip(d.zip(e)) {
        if check_free(e) && check_redoalloc(e) {
            // Freeing allocations in reverse order works so long as there are no
            // padding bytes and since the last allocations has natural alignment
            // this is the case.  But if there is padding, dealloc will fail.
            if !(check_free(e) && check_free(d) && check_free_fail(c)) {
                ok = false
            }
        } else {
            ok = false;
        }
    }

    // Reducing size of an allocation always works.
    if let Some(ref mut a) = a {
        let new_size = a.size() - 1;
        ok = check_realloc_ok(a, new_size) && ok;
    }
    // Increasing size moves allocation if it’s not the last one but just
    // succeeds if it is the last one.
    if let Some(ref mut b) = b {
        ok = check_realloc_move(b) && ok;
    }

    solana_program::log::sol_log(if ok { "SUCCESS" } else { "FAILURE" });
    u64::from(!ok) * 2
}


#[derive(Copy, Clone)]
struct Region {
    id: u8,
    ptr: *mut u8,
    layout: Layout,
}

impl Region {
    fn size(&self) -> usize { self.layout.size() }
}


/// Checks that allocation succeeds.
///
/// Verifies that the allocated region falls inside of the heap and that it
/// doesn’t overlap any previous allocations (this is done by checking that
/// region is filled with zero bytes and initialising it to `id`).
fn check_alloc(id: u8, layout: Layout) -> Option<Region> {
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() {
        log!("alloc({}, {}) failed", layout.size(), layout.align());
        return None;
    }
    let reg = Region { id, ptr, layout };
    log!("al({})", reg);
    if !check_region(reg) {
        return None;
    }

    let slice = unsafe { core::slice::from_raw_parts_mut(reg.ptr, reg.size()) };
    let err = slice.iter().map(|p| (p as *const u8, *p)).find(|&(_, v)| v != 0);
    slice.fill(id);
    if let Some((ptr, value)) = err {
        log!("#{}: conflicts with #{} at {}", id, value, ptr);
        None
    } else {
        Some(reg)
    }
}

/// Checks that region is properly aligned and falls inside of the heap.
fn check_region(reg: Region) -> bool {
    let addr = reg.ptr.addr();
    let end = addr + reg.layout.size();
    if addr < HEAP_START_ADDRESS + A.header_size() || HEAP_END_ADDRESS < end {
        log!("#{}: outside of heap", reg.id);
        false
    } else if addr % reg.layout.align() != 0 {
        log!("#{}: improperly aligned", reg.id);
        false
    } else {
        true
    }
}


/// Checks whether the global variable works correctly.
#[cfg(not(feature = "test-global"))]
const fn check_global() -> bool { true }

#[cfg(feature = "test-global")]
fn check_global() -> bool {
    let num = &A.global().num;
    let reg = Region {
        id: u8::MAX,
        ptr: core::ptr::from_ref(num).cast::<u8>().cast_mut(),
        layout: Layout::for_value(num),
    };
    log!("global{:#}", reg);

    let addr = reg.ptr.addr();
    let end = addr + reg.layout.size();
    if addr < HEAP_START_ADDRESS || HEAP_START_ADDRESS + A.header_size() < end {
        log!("global: outside of header");
        return false;
    } else if addr % reg.layout.align() != 0 {
        log!("global: improperly aligned");
        return false;
    }

    let value = num.get();
    if value != 0 {
        log!("global: not zero-initialised, got {}", value);
        return false;
    }

    num.set(u64::MAX);
    true
}


/// Checks that deallocating an object reduces memory usage, i.e. opportunistic
/// deallocation succeeds.
///
/// Verifies that used memory reported by the allocator reduces by the size of
/// the allocation.
fn check_free(reg: Region) -> bool {
    let (before, after) = do_free(reg);
    check!(
        before == after + reg.size(),
        "bad dealloc: wanted {} -> {}",
        before,
        before - reg.size()
    )
}

/// Checks that deallocating the object does not change memory usage,
/// i.e. opportunistic deallocation fails.
fn check_free_fail(reg: Region) -> bool {
    let (before, after) = do_free(reg);
    check!(before == after, "unexpected dealloc")
}

fn do_free(reg: Region) -> (usize, usize) {
    let before = A.used();
    unsafe { alloc::alloc::dealloc(reg.ptr, reg.layout) };
    let after = A.used();
    log!("de({}) used: {} -> {}", reg, before, after);
    (before, after)
}

/// Checks that allocating object right after it has been freed returns the same
/// pointer.
fn check_redoalloc(reg: Region) -> bool {
    let new_ptr = unsafe { alloc::alloc::alloc(reg.layout) };
    let new_reg = Region { ptr: new_ptr, ..reg };
    log!("al({})", new_reg);
    check_region(new_reg) &&
        check!(
            new_ptr == reg.ptr,
            "bad alloc after dealloc: {} -> {}",
            reg.ptr,
            new_ptr
        )
}


/// Checks that shrinking region works.
fn check_realloc_ok(reg: &mut Region, new_size: usize) -> bool {
    let old_ptr = reg.ptr;
    do_realloc(reg, new_size) &&
        check!(old_ptr == reg.ptr, "#{}: realloc moved", reg.id)
}

/// Checks that realloc moves region but afterwards it can be resized without
/// move.
fn check_realloc_move(reg: &mut Region) -> bool {
    let old_ptr = reg.ptr;
    if !do_realloc(reg, reg.size() + 1) ||
        !check!(old_ptr != reg.ptr, "#{}: realloc did not move", reg.id)
    {
        return false;
    }

    // Now that we allocated new region, it is the last allocation and as such
    // realloc should succeed without move.
    check_realloc_ok(reg, reg.size() + 1)
}

fn do_realloc(reg: &mut Region, new_size: usize) -> bool {
    let layout = Layout::from_size_align(new_size, reg.layout.align()).unwrap();
    let ptr = unsafe { alloc::alloc::realloc(reg.ptr, reg.layout, new_size) };
    let new_reg = Region { id: reg.id, ptr, layout };
    log!("re({} -> {:#})", *reg, new_reg);
    if !check_region(new_reg) {
        return false;
    }

    let mut res = true;
    if reg.ptr != new_reg.ptr {
        let size = new_size.min(reg.layout.size());
        let slice = unsafe { core::slice::from_raw_parts(new_reg.ptr, size) };
        if let Some(ptr) = slice.iter().find(|&ptr| *ptr != reg.id) {
            log!("#{}: got {} @ {}", reg.id, *ptr, core::ptr::from_ref(ptr));
            res = false;
        }
    }

    *reg = new_reg;
    res
}


fn do_log(args: fmt::Arguments) {
    use core::mem::MaybeUninit;
    use std::io::Write;

    struct Buffer {
        buffer: [MaybeUninit<u8>; 128],
        pos: usize,
    }

    impl Buffer {
        fn as_str(&self) -> &str {
            let slice = &self.buffer[..];
            let ptr = core::ptr::from_ref(slice) as *const [u8];
            unsafe { core::str::from_utf8_unchecked(&(*ptr)[..self.pos]) }
        }
    }

    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let len = buf.len().min(self.buffer.len() - self.pos);
            // SAFETY: &[T] and &[MaybeUninit<T>] have the same layout
            let src: &[MaybeUninit<u8>] =
                unsafe { core::mem::transmute(&buf[..len]) };
            self.buffer[self.pos..][..len].copy_from_slice(src);
            self.pos += len;
            Ok(len)
        }

        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    let used = Arg::from(A.used());
    let offset = Arg::from(A.get_end_offset());

    let mut buffer = Buffer { buffer: [MaybeUninit::uninit(); 128], pos: 0 };
    write!(&mut buffer, "{args}; used: {used}, offset: {offset}").unwrap();
    solana_program::log::sol_log(buffer.as_str());
}

#[derive(derive_more::From)]
enum Arg {
    Num(usize),
    Reg(Region),
}

impl Arg {
    fn fmt_num(n: usize, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut buffer = itoa::Buffer::new();
        fmt.write_str(buffer.format(n))
    }

    fn fmt_reg(r: &Region, fmt: &mut fmt::Formatter) -> fmt::Result {
        if !fmt.alternate() {
            write!(fmt, "#{}", Arg::from(r.id))?;
        }
        write!(
            fmt,
            "<{}..{}; {}B/{}B>",
            Arg::from(r.ptr),
            Arg::from(r.ptr.wrapping_add(r.size())),
            Arg::from(r.size()),
            Arg::from(r.layout.align())
        )
    }
}

impl fmt::Display for Arg {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Num(n) => Arg::fmt_num(*n, fmt),
            Self::Reg(r) => Arg::fmt_reg(r, fmt),
        }
    }
}

impl From<u8> for Arg {
    fn from(v: u8) -> Arg { Arg::Num(usize::from(v)) }
}
impl From<u64> for Arg {
    fn from(v: u64) -> Arg { Arg::Num(v as usize) }
}
impl From<*const u8> for Arg {
    fn from(ptr: *const u8) -> Arg { Arg::Num(ptr.addr() - HEAP_START_ADDRESS) }
}
impl From<*mut u8> for Arg {
    fn from(ptr: *mut u8) -> Arg { Arg::Num(ptr.addr() - HEAP_START_ADDRESS) }
}


#[cfg(not(feature = "test-global"))]
solana_allocator::custom_heap!();
#[cfg(feature = "test-global")]
solana_allocator::custom_global!(
    struct GlobalData {
        num: core::cell::Cell<u64>,
    }
);


solana_program::custom_panic_default!();
