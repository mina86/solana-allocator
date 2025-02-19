# solana-allocator

A custom Solana allocator with the following features:

- Supports heaps of arbitrary size.  Default Solana allocator assumes
  32 KiB of available space regardless of heap size configured via the
  `ComputeBudgetInstruction`.  This allocator will use all the
  allotted space.

- Supports opportunistic freeing and resizing.  The default Solana
  allocator never frees memory and calling `msg!` in a loop will
  eventually exhaust all memory.  This allocator frees at least the
  last allocated object addressing simple case of temporary buffers.

- Allows declaring mutable global variables which is something Solana
  Virtual Machine doesn’t normally support.  (Any program with mutable
  static variable will fail to deploy).  This allocator offers a way
  to define such state.

## Usage

The crate provides `custom_heap` and `custom_global` macros which make
use of the allocator relatively straightforward.  Note however, that
`solana_program::entrypoint` and `anchor::program` macros define
global allocator of their own unless `custom-heap` Cargo feature is
enabled.

### Simple usage

For a simple usage (without support for mutable static variables), add
the dependency and use `custom_heap` macro.  First in `Cargo.toml`:

```toml
[dependencies.solana-allocator]
git = "https://github.com/mina86/solana-allocator"
optional = true

[features]
default = ["custom-heap"]
custom-heap = ["dep:solana-allocator"]
```

And than in `lib.rs`:

```rust
#[cfg(feature = "custom-heap")]
solana_allocator::custom_heap();
```

### Usage with mutable global variables

For usage with the mutable global variables, additional `bytemuck`
dependency must be added and `custom_global` macro used instead.
First in `Cargo.toml`:

```toml
[dependencies.bytemuck]
version = "*"
optional = true

[dependencies.solana-allocator]
git = "https://github.com/mina86/solana-allocator"
optional = true

[features]
default = ["custom-heap"]
custom-heap = ["dep:bytemuck", "dep:solana-allocator"]
```

And then in `lib.rs`, for example:

```rust
solana_allocator::custom_global!(struct GlobalData {
    counter: Cell<usize>,
});

#[cfg(target_os = "solana")]
pub fn unique() -> usize {
    let counter = &global().counter;
    let value = counter.get();
    counter.set(value + 1);
    value
}

#[cfg(not(target_os = "solana"))]
pub fn unique() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}
```
