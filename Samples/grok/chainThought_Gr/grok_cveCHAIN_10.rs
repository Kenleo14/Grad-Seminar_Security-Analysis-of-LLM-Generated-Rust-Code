// Cargo.toml equivalent for demo:
// [package] name = "binder_fix" version = "0.1.0"
// [dependencies]

//! Complete fixed module modeling rust_binder Node todo/stack lists.
//! Compiles/runs with `cargo run`. Tests concurrency safety.

use std::cell::UnsafeCell;
use std::fmt;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

// Kernel-style alignment/packing for UnsafeCell.
#[repr(C)]
#[repr(align(8))]
pub struct NodeLinks {
    prev: UnsafeCell<Option<NonNull<Node>>>,
    next: UnsafeCell<Option<NonNull<Node>>>,
}

// SAFETY: Send/Sync if no self-referential unsync access (lock protects).
unsafe impl Send for NodeLinks {}
unsafe impl Sync for NodeLinks {}

pub struct Node {
    pub links: NodeLinks,
    pub data: usize, // Dummy payload.
}

// Dummy impls (kernel would have refcount, etc.).
impl Node {
    pub fn new(data: usize) -> Self {
        Self {
            links: NodeLinks {
                prev: UnsafeCell::new(None),
                next: UnsafeCell::new(None),
            },
            data,
        }
    }

    /// BUGGY version (CVE): Drops lock BEFORE unsafe remove.
    /// Concurrent move_to_stack races on links.
    /// 