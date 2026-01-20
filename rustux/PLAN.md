# Rustux Development Plan

## Current Status: Heap Allocator Bug Fix in Progress

### The Bug

During ELF loading, the vaddr field of Segment structures is being corrupted to `0x300028` (heap_base + sizeof(BlockHeader)).

### Root Cause Analysis

The heap allocator (`src/mm/allocator.rs`) has a critical flaw in its allocation strategy:

1. **First Allocation (segments Vec)**:
   - Allocator finds the 16MB free block starting at 0x300000
   - Removes block from free_list (free_list becomes null)
   - Allocates memory for segments Vec
   - Block splitting logic: `if remaining >= MIN_BLOCK_SIZE * 4` (160 bytes)
   - If remaining space < 160 bytes, no new free block is created
   - Result: free_list is now null

2. **Second Allocation (Box<LoadedElf>)**:
   - Allocator searches free_list
   - free_list is null (empty)
   - Returns null pointer
   - Allocation fails

### The Corruption Pattern

Why vaddr becomes 0x300028:
- `heap_base = 0x300000`
- `sizeof(BlockHeader) = 40 bytes` (0x28)
- First allocation's payload starts at `0x300000 + 40 = 0x300028`
- When Box<LoadedElf> fails to allocate, uninitialized memory or zero values may cause the observed corruption

### Fixes Implemented

1. **Kernel Stack Switch** (COMPLETE):
   - Location: `src/arch/amd64/init.rs`
   - Allocates 256KB (64 pages) for kernel stack
   - Uses non-returning jump to continuation
   - Prevents stack overflow on UEFI's small 4-8KB firmware stack

2. **Heap Allocator Improvements** (IN PROGRESS):
   - Location: `src/mm/allocator.rs`
   - Added pointer clearing when blocks are allocated
   - Added LAST_ALLOCATED tracking to prevent immediate address reuse
   - Fixed block splitting alignment

### Remaining Work

1. **Fix Block Splitting Logic**:
   - Current threshold (MIN_BLOCK_SIZE * 4 = 160 bytes) may be too high
   - Need to ensure small remaining blocks are still added to free_list
   - Alternative: Consider using a simpler bump allocator for early boot

2. **Reduce Memory Usage**:
   - The segments Vec allocation may be reserving too much capacity
   - Consider using a fixed-size array for small numbers of segments

3. **Test the Fix**:
   - Build and run the test ELF
   - Verify vaddr is no longer corrupted to 0x300028

### Next Steps

1. Fix the block splitting threshold or implement bump allocator
2. Test with current ELF binary
3. Verify all segments load correctly
4. Commit working implementation
