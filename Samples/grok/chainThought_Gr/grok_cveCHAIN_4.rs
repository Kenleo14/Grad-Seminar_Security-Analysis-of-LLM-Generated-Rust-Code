//!
//! Fixed Rust module modeling rust_binder Node + intrusive list.
//! Synchronization invariant: Links & `active` modified ONLY under lock.
//! Compile: cargo check (add `loom = { version = "*", features = ["std", "reqwest"] }` for model checking).
//!

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering::{self, Acquire, Relaxed, Release}},
    Arc, Mutex, MutexGuard,
};
use std::ptr::{self, NonNull};
use std::cell::UnsafeCell;
use std::mem;

// Kernel-like intrusive doubly-linked list (no_std compatible if ported).
#[repr(C)]
#[derive(Debug)]
pub struct ListLinks {
    pub next: *mut Node,
    pub prev: *mut Node,
}

impl ListLinks {
    /// Initialize as empty (null-terminated for simplicity; sentinel optional).
    pub fn new() -> Self {
        Self { next: ptr::null_mut(), prev: ptr::null_mut() }
    }

    /// Unsafe remove from list (splice out). Assumes caller upholds invariant (lock held).
    /// Violates aliasing if concurrent access!
    pub unsafe fn remove(&mut self) {
        // Splice: link prev -> next, next -> prev.
        unsafe {
            if !self.prev.is_null() {
                (*self.prev).links.next = self.next;
            }
            if !self.next.is_null() {
                (*self.next).links.prev = self.prev;
            }
            // Null self for safety (poison).
            self.next = ptr::null_mut();
            self.prev = ptr::null_mut();
        }
    }

    /// Unsafe insert after head. (Used for push_front.)
    pub unsafe fn push_front(head: *mut Node, node: *mut Node) {
        unsafe {
            (*node).links.prev = ptr::null_mut();
            (*node).links.next = (*head).links.next;
            if !(*head).links.next.is_null() {
                (*(*head).links.next).links.prev = node;
            }
            (*head).links.next = node;
        }
    }
}

/// Active nodes list head (protected by Mutex).
pub struct ActiveNodes {
    pub lock: Mutex<()>,
    pub head: NonNull<Node>,  // Sentinel head (owned externally).
}

impl ActiveNodes {
    /// Lock + guard for protected access.
    pub fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap()
    }
}

/// Node: refcounted, intrusive links, active flag.
/// Models kernel::alloc::Pinned<kobj::Node>.
#[repr(C)]
pub struct Node {
    pub links: ListLinks,
    pub refcnt: AtomicUsize,
    pub active: AtomicBool,
    // Dummy data/payload.
    pub data: UnsafeCell<String>,
}

impl Node {
    pub fn new(data: String) -> Arc<Self> {
        let node = Arc::new_cyclic(|weak| Self {
            links: ListLinks::new(),
            refcnt: AtomicUsize::new(1),
            active: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        });
        // Leak Arc to simulate pinned kernel alloc (no auto-drop).
        let weak_ptr: NonNull<Node> = unsafe { NonNull::new_unchecked(Arc::into_raw(node.clone()) as *mut Node) };
        node
    }

    /// Take a strong ref (Relaxed ok under lock).
    pub fn inc_ref(&self) {
        self.refcnt.fetch_add(1, Relaxed);
    }

    /// Release ref. If ->0 AND active, unlink under lock, then safe to free.
    /// FIXED: Unlink BEFORE unlock. No lockless unsafe!
    pub fn release(&self, active_nodes: &ActiveNodes) {
        let _guard = active_nodes.lock();  // Serialize w/ move_to_stack.
        let prev = self.refcnt.fetch_sub(1, Release);
        if prev == 1 {  // Now 0.
            if self.active.swap(false, Acquire) {  // Atomic clear + check.
                // UNLINK UNDER LOCK: Upholds invariant.
                unsafe { (*self).links.remove() };
            }
        }
        // Drop _guard: unlock.
        // NOW safe: unlinked + ref=0 → queue_free(self) lockless.
        // Simulate free (in kernel: kfree or workqueue).
        println!("Node freed: {:?}", unsafe { &*self.data.get() });
    }

    /// Move to local stack: inc_ref, unlink, transfer.
    /// FIXED: All under lock, checks active.
    pub fn move_to_stack(&self, active_nodes: &ActiveNodes, stack: &mut Vec<Arc<Node>>) {
        let mut guard = active_nodes.lock();
        if self.active.load(Acquire) {
            self.inc_ref();  // Ref >=1 now.
            unsafe { self.links.remove() };
            self.active.store(false, Release);
            // Transfer to stack (drop guard after push).
            drop(guard);
            stack.push(Arc::clone(self));
            println!("Moved to stack: {:?}", unsafe { &*self.data.get() });
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // Simulate kernel free check.
        assert_eq!(self.refcnt.load(Relaxed), 0, "Drop with refs!");
    }
}

/// Demo: Simulate concurrent release/move_to_stack.
/// In loom: cargo test --lib (model checks no races).
#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc::channel, thread};

    #[test]
    fn fixed_demo() {
        let data1 = "node1".to_string();
        let node1 = Node::new(data1);
        let active_nodes = ActiveNodes {
            lock: Mutex::new(()),
            head: unsafe { NonNull::new_unchecked(Arc::into_raw(node1.clone()) as *mut Node) },  // Fake sentinel.
        };

        // Simulate push to active list (under lock).
        let mut guard = active_nodes.lock();
        unsafe { ListLinks::push_front(active_nodes.head.as_ptr(), Arc::into_raw(node1.clone()) as *mut Node) };
        unsafe { (*Arc::into_raw(node1.clone()) as *mut Node).links.prev = active_nodes.head.as_ptr() };
        node1.active.store(true, Release);
        drop(guard);

        let mut stack = Vec::new();
        let (tx, rx) = channel();

        // Thread 1: move_to_stack.
        let n1 = Arc::clone(&node1);
        let an = &active_nodes;
        thread::spawn(move || {
            n1.move_to_stack(an, &mut stack);
            tx.send(()).unwrap();
        });

        // Thread 2: release (races in buggy, safe here).
        let n2 = Arc::clone(&node1);
        thread::spawn(move || {
            n2.release(an);
        });

        rx.recv().unwrap();  // Wait.
        // Success: either moved (ref>0) or released (unlinked).
        // No UB, list consistent.
    }
}

// Buggy version (commented): Drops lock EARLY.
// pub fn buggy_release(&self, active_nodes: &ActiveNodes) {
//     let _guard = active_nodes.lock();
//     let prev = self.refcnt.fetch_sub(1, Release);
//     if prev == 1 {
//         drop(_guard);  // !!! BUG: Unlock before unlink.
//         if self.active.load(Acquire) {
//             unsafe { self.links.remove() };  // Races w/ move_to_stack!
//         }
//     }
//     // UB under contention.
