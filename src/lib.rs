// solana-allocator — custom global allocator for Solana programs.
// © 2024 by Composable Foundation
// © 2025 by Michał Nazarewicz <mina86@mina86.com>
//
// This program is free software; you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation; either version 2 of the License, or (at your option) any later
// version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more
// details.
//
// You should have received a copy of the GNU General Public License along with
// this program; if not, see <https://www.gnu.org/licenses/>.

// Rust doesn’t recognise ‘solana’ as a target_os unless building via cargo
// build-sbf.  Silence the warning.
#![cfg_attr(not(target_os = "solana"), allow(unexpected_cfgs))]
#![allow(private_bounds)]

//! Custom global allocator which doesn’t assume 32 KiB heap size.
//!
//! Default Solana allocator assumes there’s only 32 KiB of available heap
//! space.  Since heap size can be changed per-transaction, this assumption is
//! not always accurate.  This module defines a global allocator which doesn’t
//! assume size of available space.

extern crate alloc;

#[cfg(any(test, target_os = "solana"))]
mod imp;
#[cfg(any(test, target_os = "solana"))]
mod ptr;

#[cfg(any(test, target_os = "solana"))]
pub use imp::BumpAllocator;


/// On Solana, defines `BumpAllocator` as the global allocator.
///
/// When compiling for Solana, defines a new instance of `BumpAllocator` and
/// declares it as a the global allocator.  When compiling for other platforms,
/// does nothing.
///
/// Note that, as always when defining custom global allocator on Solana, if
/// using `solana_program::entrypoint` or `anchor::program` macros, the smart
/// contract must define and enable `custom-heap` feature.  Otherwise, global
/// allocator defined by this macro and in `solana_program` will clash.
///
/// # Example
///
/// ```ignore
/// #[cfg(not(feature = "cpi"))]
/// solana_allocator::custom_heap!();
/// ```
#[macro_export]
macro_rules! custom_heap {
    () => {
        #[cfg(target_os = "solana")]
        #[global_allocator]
        // SAFETY: We’re compiling for Solana and declaring this as a global
        // allocator which can exist only one.
        static A: $crate::BumpAllocator<()> = unsafe {
            $crate::BumpAllocator::new();
        };
    };
}


/// On Solana, defines `BumpAllocator` as the global allocator with given
/// global state.
///
/// When compiling for Solana, defines a new global allocator and a function
/// which returns static object living in mutable memory.  The name of the
/// function and type of the global object depend on the invocation.
///
/// See also caveats in [`custom_heap`] macro.
///
/// # Existing type
///
/// ```ignore
/// custom_global!($visibility fn $name() -> $Global);
/// custom_global!($visibility type $Global);
/// ```
///
/// Defines function `$name` with specified visibility which returns a reference
/// to a `'static` value of type `$Global`.  In the second invocation, the name
/// of the function is `global`.
///
/// `$Global` must be a [`Sync`](`core::marker::Sync`) and
/// [`bytemuck::Zeroable`] type.  Furthermore, the `$name` function returns
/// a shared reference to the static objects which doesn’t allow modification
/// unless the type has internal mutability.  This can be achieved by
/// [`Cell`](`core::cell::Cell`).
///
/// # Global struct definition
///
/// ```ignore
/// custom_global!($visibility fn $name() -> struct $Global { ... });
/// custom_global!($visibility struct $Global { ... });
/// ```
///
/// Defines a struct `$Global` and uses that as the global object.  Note that
/// all fields of the struct must be [`bytemuck::Zeroable`] *however* they do
/// not need to be [`Sync`](`core::marker::Sync`).  When building on Solana, the
/// macro will unsafely declare `$Global` as `Sync` based on the observation
/// that Solana is single-threaded thus passing data between threads is not
/// a concern.
///
/// # Non-Solana target
///
/// When not building for Solana (i.e. for `not(target_os = "solana")`
/// configuration), the macro doesn’t set the global allocator nor defines the
/// `$name` function returning the global state.  (Note that the invocation with
/// `struct $Global` definition defines the struct regardless of the target).
///
/// Caller is responsible for using appropriate conditional compilation to
/// provide all the necessary global state.  One approach is to provide wrapper
/// functions which use the global object when building on Solana and use static
/// atomic type or locked variables when building on other platforms.
///
/// # Example
///
/// ```ignore
/// pub(crate) mod global {
///     #[cfg(not(feature = "cpi"))]
///     solana_allocator::custom_global!(struct GlobalData {
///         counter: Cell<usize>,
///     });
///
///     #[cfg(all(target_os = "solana", not(feature = "cpi")))]
///     pub fn unique() -> usize {
///         let counter = &global().counter;
///         let value = counter.get();
///         counter.set(value + 1);
///         value
///     }
///
///     #[cfg(not(target_os = "solana"))]
///     pub fn unique() -> usize {
///         use std::sync::atomic::{AtomicUsize, Ordering};
///         static COUNTER: AtomicUsize = AtomicUsize::new(0);
///         COUNTER.fetch_add(1, Ordering::SeqCst)
///     }
/// }
/// ```
#[macro_export]
macro_rules! custom_global {
    ($visibility:vis fn $name:ident() -> $G:ty) => {
        #[cfg(target_os = "solana")]
        $visibility fn $name() -> &'static $G {
            #[global_allocator]
            // SAFETY: We’re compiling for Solana and declaring this as a global
            // allocator which can exist only one.
            static A: $crate::BumpAllocator<$G> = unsafe {
                $crate::BumpAllocator::new()
            };

            A.global()
        }
    };

    ($visibility:vis type $G:ty) => {
        $crate::custom_global!($visibility fn global() -> $G);
    };

    ($visibility:vis fn $name:ident() -> struct $G:ident { $($tt:tt)* }) => {
        #[derive(bytemuck::Zeroable)]
        $visibility struct $G { $($tt)* }

        // SAFETY: $G might not be Sync.  However, Solana is single-threaded so
        // we don’t need to worry about thread safety.  Since this
        // implementation is used when building for Solana, we can safely lie to
        // the compiler about $G being Sync.
        //
        // We need $G to be Sync because it’s !Sync status percolates to
        // BumpAllocator<$G> and since that’s a static variable, Rust requires
        // that it’s Sync.
        #[cfg(target_os = "solana")]
        unsafe impl core::marker::Sync for $G {}

        $crate::custom_global!($visibility fn $name() -> $G);
    };

    ($visibility:vis struct $G:ident { $($tt:tt)* }) => {
        $crate::custom_global!(
            $visibility fn global() -> struct $G { $($tt)* }
        );
    }
}
