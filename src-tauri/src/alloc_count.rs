//! Test-only heap-allocation counter.
//!
//! Some optimizations are about what never gets copied. A timing assertion for
//! that is flaky, so the counter gives the behaviour a deterministic witness:
//! a test publishes the address of its own counter, and only allocations on the
//! publishing thread are counted. Other test threads running in parallel are
//! invisible to it, which is what makes the assertion safe inside the ordinary
//! `cargo test` run.
//!
//! The thread slot is a `const`-initialized `Cell` holding a bare pointer with
//! no destructor, so reading it from the global allocator neither allocates nor
//! runs lazy initialization.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    /// Address of the counting thread's counter, null when this thread is not
    /// measuring anything.
    static COUNTER: Cell<*const AtomicUsize> = const { Cell::new(ptr::null()) };
}

/// RAII guard: while it lives, allocations on this thread bump `count`.
pub(crate) struct Measured {
    count: *const AtomicUsize,
}

impl Measured {
    /// How many allocations have been seen so far for this measurement.
    pub(crate) fn count(&self) -> usize {
        unsafe { &*self.count }.load(Ordering::Relaxed)
    }
}

impl Drop for Measured {
    fn drop(&mut self) {
        COUNTER.with(|c| c.set(ptr::null()));
    }
}

/// Start counting allocations on the current thread; the counter is zeroed
/// first so a reused `static` measures exactly one guarded region.
pub(crate) fn measure(count: &'static AtomicUsize) -> Measured {
    count.store(0, Ordering::Relaxed);
    COUNTER.with(|c| c.set(count as *const AtomicUsize));
    Measured { count }
}

pub(crate) struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[inline(always)]
fn bump() {
    // `try_with` does not initialize the slot for a thread that never created
    // it; a null slot costs one compare.
    let _ = COUNTER.try_with(|c| {
        if let Some(counter) = unsafe { c.get().as_ref() } {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });
}
