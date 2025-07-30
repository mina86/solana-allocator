# Figuring out heap size in Solana

Code for extracting the `ComputeBudgetInstruction::RequestHeapFrame` instruction
from Solana program input.  The `extract_heap_size` function should be called in
the `entrypoint` function and returned value can be used to initialises a global
allocator.

Observe that `solana_program::entrypoint!` and `anchor::program!` macros cause
memory allocations prior to the moment user-code gets its turn so using them
requires the global allocator to be initialised from the start of the program.

This may limit the designs of the global allocator since at the program start
the allocator doesn’t know (and has no way of figuring out) the actual size of
the heap.  Heap is guaranteed to be at least 32 KiB but it can be increased
through an invocation of the Compute Budget Program.

With code presented here it is possible to implement one’s own `entrypoint`
function which first calls `extract_heap_size` to figure out the heap size, then
initialises the global allocator and finally uses
`solana_program::entrypoint::deserialize` to parse the input and begin
processing.
