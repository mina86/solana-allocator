#![cfg(target_os = "solana")]

#[no_mangle]
pub unsafe extern "C" fn entrypoint(_input: *mut u8) -> u64 { ret() }

#[cfg(not(feature = "global"))]
fn ret() -> u64 { 0 }

#[cfg(feature = "global")]
fn ret() -> u64 { return global().num.get() }

#[cfg(not(feature = "global"))]
solana_allocator::custom_heap!();

#[cfg(feature = "global")]
solana_allocator::custom_global!(
    struct GlobalData {
        num: core::cell::Cell<u64>,
    }
);

#[no_mangle]
fn custom_panic(_info: &core::panic::PanicInfo) { }
