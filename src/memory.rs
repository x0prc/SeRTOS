use crate::sync;
use core::ptr::NonNull;

pub struct FixedBlockAllocator<const BLOCK_SIZE: usize, const BLOCK_COUNT: usize> {
    // Fixed-capacity storage keeps allocation time deterministic and avoids any
    // dependence on a general-purpose heap inside the kernel core.
    blocks: [[u8; BLOCK_SIZE]; BLOCK_COUNT],
    used: [bool; BLOCK_COUNT],
}

impl<const BLOCK_SIZE: usize, const BLOCK_COUNT: usize>
    FixedBlockAllocator<BLOCK_SIZE, BLOCK_COUNT>
{
    pub const fn new() -> Self {
        Self {
            blocks: [[0; BLOCK_SIZE]; BLOCK_COUNT],
            used: [false; BLOCK_COUNT],
        }
    }

    pub const fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    pub const fn block_capacity(&self) -> usize {
        BLOCK_COUNT
    }

    pub fn available_blocks(&self) -> usize {
        self.used.iter().filter(|slot| !**slot).count()
    }

    pub fn alloc(&mut self) -> Option<NonNull<u8>> {
        sync::with(|_| {
            for index in 0..BLOCK_COUNT {
                if self.used[index] {
                    continue;
                }

                self.used[index] = true;
                return NonNull::new(self.blocks[index].as_mut_ptr());
            }

            None
        })
    }

    pub fn free(&mut self, ptr: NonNull<u8>) -> bool {
        sync::with(|_| {
            let ptr = ptr.as_ptr() as usize;

            for index in 0..BLOCK_COUNT {
                let base = self.blocks[index].as_ptr() as usize;
                if ptr != base {
                    continue;
                }

                if !self.used[index] {
                    return false;
                }

                self.used[index] = false;
                // Clearing the block makes stale data reuse easier to spot while
                // keeping the allocator behavior deterministic.
                self.blocks[index].fill(0);
                return true;
            }

            false
        })
    }

    pub fn block_index(&self, ptr: NonNull<u8>) -> Option<usize> {
        let ptr = ptr.as_ptr() as usize;

        for index in 0..BLOCK_COUNT {
            if ptr == self.blocks[index].as_ptr() as usize {
                return Some(index);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::FixedBlockAllocator;

    #[test]
    fn allocator_reuses_freed_block() {
        let mut allocator = FixedBlockAllocator::<16, 2>::new();
        let first = allocator.alloc().expect("first block");
        let second = allocator.alloc().expect("second block");
        assert!(allocator.alloc().is_none());
        assert!(allocator.free(first));
        let third = allocator.alloc().expect("reused block");
        assert_eq!(allocator.block_index(first), allocator.block_index(third));
        assert!(allocator.free(second));
        assert!(allocator.free(third));
    }
}
