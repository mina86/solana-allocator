#![cfg(target_os = "solana")]


#[cfg(feature = "custom-heap")]
#[cfg(not(any(feature = "bump-alloc", feature = "ll-alloc")))]
compile_error!("Must enable bump-alloc or ll-alloc if enabling custom-heap");

#[cfg(all(feature = "bump-alloc", feature = "ll-alloc"))]
compile_error!("Cannot enable bump-alloc and ll-alloc both");


/// Address of the start of Solana heap.
#[allow(dead_code)]
const HEAP_START_ADDRESS: u64 = 0x3_0000_0000;

/// Size of the Solana heap.  The smart contract is run with heap increased from
/// the default 32K.
#[allow(dead_code)]
const HEAP_LENGTH: u64 = 256 * 1024;

/// Layout of the small objects we’re allocating.
const LAYOUT: std::alloc::Layout = {
    // If we’re testing freeing, make sure there are no padding bytes.
    // Otherwise leave padding bytes in case it affects the allocation time
    // (though I don’t see how it would).
    let size = if cfg!(feature = "free") { 4 } else { 3 };
    let align = 2;
    match std::alloc::Layout::from_size_align(size, align) {
        Ok(x) => x,
        Err(_) => panic!(),
    }
};

/// Number of small objects we’re allocating.
///
/// The number depends on the type of benchmark and allocator used.
const COUNT: usize = if cfg!(feature = "small-count") {
    100
} else {
    let kind: u32 = if !cfg!(feature = "custom-heap") {
        0
    } else if cfg!(feature = "bump-alloc") {
        1
    } else if cfg!(feature = "ll-alloc") {
        2
    } else {
        panic!();
    };
    #[rustfmt::skip]
    let count: usize = match (kind, cfg!(feature = "free")) {
        // Default allocator can use only 32K
        (0, _)     => (32 * 1024 - 8) / 4,
        // For other allocators, we may need to limit the count to avoid running
        // out of CU.
        (1, false) => 196 * 1024 / 4,
        (1, true)  => 128 * 1024 / 4,
        (2, false) => 196 * 1024 / 16,
        (2, true)  =>  96 * 1024 / 16,
        (_, _)     => panic!(),
    };
    count - 1
};


/// Emits a Solana log message without performing memory allocations.
macro_rules! log {
    ($fmt:literal $(, $args:expr)* $(,)?) => {{
        let alloc = if !cfg!(feature = "custom-heap") {
            "default   "
        } else if cfg!(feature = "ll-alloc") {
            "ll-alloc  "
        } else {
            "bump-alloc"
        };
        do_log(format_args!(concat!("{} ", $fmt), alloc $(, $args)*));
    }}
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
#[cfg(not(feature = "free"))]
#[no_mangle]
pub unsafe extern "C" fn entrypoint(_input: *mut u8) -> u64 {
    // Measure no-operation to account for overhead of the measurement.
    let nop_time = measure(|| ());

    // Measure first allocation separately since it includes initialisation.
    let mut ptr = core::ptr::null_mut();
    let first_time = measure(|| ptr = unsafe { std::alloc::alloc(LAYOUT) });
    assert!(!ptr.is_null());

    log!("nop: {:>3}; first: {:>3}", Num(nop_time), Num(first_time));

    let mut last: *mut u8 = core::ptr::null_mut();
    let alloc_time = measure(|| {
        for _ in 1..COUNT {
            let _ = unsafe { std::alloc::alloc(LAYOUT) };
        }
        last = unsafe { std::alloc::alloc(LAYOUT) };
    });
    assert!(!last.is_null());
    let used = alloc::used();

    log!(
        "alloc: {:>7}/{:>5};                         used: {:>6}",
        Num(alloc_time),
        Num(COUNT),
        Num(used)
    );
    solana_program::entrypoint::SUCCESS
}

#[cfg(feature = "free")]
#[no_mangle]
pub unsafe extern "C" fn entrypoint(_input: *mut u8) -> u64 {
    // Don’t measure first allocation  (since it initialises the allocator).
    let first = unsafe { std::alloc::alloc(LAYOUT) } as usize;
    assert!(first != 0);

    let (mut second, mut last) = (0, 0);
    let alloc_time = measure(|| {
        second = unsafe { std::alloc::alloc(LAYOUT) } as usize;
        for _ in 2..COUNT {
            let _ = unsafe { std::alloc::alloc(LAYOUT) };
        }
        last = unsafe { std::alloc::alloc(LAYOUT) } as usize;
    });
    assert!(second != 0 && last != 0);
    let used_1 = alloc::used();

    // We take advantage of the fact that we’re allocating all the same objects.
    // This means that distance in address between each two object will be
    // `first - zeroth`.  Nth allocated object is at position `first + N *
    // (first - zeroth)`; or counting from the back `last + N * (zeroth -
    // first)`.
    let (mut ptr, step, free_op) = if cfg!(feature = "rev-free") {
        (last, first.wrapping_sub(second), "rev-free")
    } else {
        (first, second.wrapping_sub(first), "free    ")
    };

    let free_time = measure(|| {
        for _ in 0..COUNT {
            unsafe {
                std::alloc::dealloc(ptr as *mut u8, LAYOUT);
            }
            ptr = ptr.wrapping_add(step);
        }
    });
    let used_2 = alloc::used();

    log!(
        "alloc: {:>7}/{:>5}; {}: {:>6}/{:>5}; used: {:>6}, {:>6}",
        Num(alloc_time),
        Num(COUNT),
        free_op,
        Num(free_time),
        Num(COUNT),
        Num(used_1),
        Num(used_2),
    );
    solana_program::entrypoint::SUCCESS
}


fn do_log(args: core::fmt::Arguments) {
    use core::mem::MaybeUninit;
    use std::io::Write;

    struct Buffer {
        buffer: [MaybeUninit<u8>; 128],
        pos: usize,
    }

    impl Buffer {
        fn as_str(&self) -> &str {
            let slice = &self.buffer[..];
            let ptr = slice as *const [MaybeUninit<u8>] as *const [u8];
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

    let mut buffer = Buffer { buffer: [MaybeUninit::uninit(); 128], pos: 0 };
    buffer.write_fmt(args).unwrap();
    solana_program::log::sol_log(buffer.as_str());
}

struct Num(usize);

impl core::fmt::Display for Num {
    fn fmt(&self, fmt: &mut core::fmt::Formatter) -> core::fmt::Result {
        static SPACES: &str = "        ";
        let mut buffer = itoa::Buffer::new();
        let printed = buffer.format(self.0);
        let mut pad = fmt.width().unwrap_or(0).saturating_sub(printed.len());
        while pad > 0 {
            let p = pad.min(SPACES.len());
            fmt.write_str(&SPACES[..p])?;
            pad -= p;
        }
        fmt.write_str(printed)
    }
}


/// Measure how much CU executing `body` takes.
#[inline(always)]
fn measure(body: impl FnOnce()) -> usize {
    let start = solana_program::compute_units::sol_remaining_compute_units();
    body();
    let end = solana_program::compute_units::sol_remaining_compute_units();
    (start - end) as usize
}


#[cfg(not(feature = "custom-heap"))]
mod alloc {
    solana_program::custom_heap_default!();

    pub fn used() -> usize {
        let ptr = 0x3_0000_0000 as *const core::cell::Cell<u64>;
        match unsafe { &*ptr }.get() {
            0 => 0,
            addr => (32 * 1024 - (addr & 0xffff_ffff)) as usize,
        }
    }
}

#[cfg(feature = "bump-alloc")]
mod alloc {
    solana_allocator::custom_heap!();

    #[cfg(feature = "bump-alloc")]
    pub fn used() -> usize { A.used() }
}

#[cfg(feature = "ll-alloc")]
mod alloc {
    use std::alloc::Layout;

    use linked_list_allocator::Heap;

    use super::{HEAP_LENGTH, HEAP_START_ADDRESS};

    #[global_allocator]
    static ALLOC: LLAlloc = LLAlloc;

    struct LLAlloc;

    impl LLAlloc {
        fn heap() -> &'static mut Heap {
            let heap: *mut Heap = (HEAP_START_ADDRESS as *mut u8).cast();
            let heap: &mut Heap = unsafe { &mut *heap };
            if heap.bottom().is_null() {
                let head_size = core::mem::size_of_val(heap) as u64;
                let start = HEAP_START_ADDRESS + head_size;
                let size = HEAP_LENGTH - head_size;
                unsafe { heap.init(start as *mut u8, size as usize) }
            }
            heap
        }
    }

    pub fn used() -> usize { LLAlloc::heap().used() }

    unsafe impl std::alloc::GlobalAlloc for LLAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            match Self::heap().allocate_first_fit(layout) {
                Ok(ptr) => ptr.as_ptr(),
                Err(_) => core::ptr::null_mut(),
            }
        }

        /// Deallocates specified object.
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            let ptr = unsafe { core::ptr::NonNull::new_unchecked(ptr) };
            Self::heap().deallocate(ptr, layout)
        }
    }
}


solana_program::custom_panic_default!();
