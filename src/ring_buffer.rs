use core::mem::MaybeUninit;

pub struct RingBuffer<T, const N: usize> {
    // Elements live in MaybeUninit slots so the buffer can reserve fixed storage
    // up front without requiring `T: Copy` or eagerly constructing `N` values.
    storage: [MaybeUninit<T>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T, const N: usize> RingBuffer<T, N> {
    pub const fn new() -> Self {
        Self {
            storage: [const { MaybeUninit::uninit() }; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == N
    }

    pub fn push(&mut self, value: T) -> Result<(), T> {
        if self.is_full() {
            return Err(value);
        }

        // Tail always points at the next vacant slot because `len` tracks how
        // many initialized elements currently occupy the circular buffer.
        self.storage[self.tail].write(value);
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        // The head slot is guaranteed initialized whenever `len > 0`, so moving
        // the value out with `assume_init_read` is valid here.
        let value = unsafe { self.storage[self.head].assume_init_read() };
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(value)
    }
}

impl<T, const N: usize> Drop for RingBuffer<T, N> {
    fn drop(&mut self) {
        // Drain any still-initialized elements so their destructors run exactly
        // once before the backing MaybeUninit storage goes away.
        while self.pop().is_some() {}
    }
}
