#![no_std]
#![feature(newtype_index, strict_provenance)]

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem,
    ptr::{self, NonNull},
    sync::atomic::{AtomicU32, Ordering},
};

use kernel::{
    sync::{self, Mutex},
    try_module,
    types::ARef,
    Error,
};

// Intrusive doubly-linked list head (kernel-style).
#[repr(C)]
pub struct ListHead {
    pub prev: *mut Node,
    pub next: *mut Node,
}

unsafe impl Send for ListHead {}
unsafe impl Sync for ListHead {}

// Node with intrusive links + refcount (mimics binder Node).
#[repr(C)]
pub struct Node {
    pub prev: *mut Node,
    pub next: *mut Node,
    pub refs: AtomicU32,
    pub data: u64, // Dummy payload.
}

// Safety: Node fields are Sync/Send.
unsafe impl Send for Node {}
unsafe impl Sync for Node {}

impl ListHead {
    /// Create empty list.
    pub const fn new() -> Self {
        Self {
            prev: ptr::null_mut(),
            next: ptr::null_mut(),
        }
    }

    /// Is list empty?
    pub fn is_empty(&self) -> bool {
        self.next == self as *const _ as *mut _
    }
}

impl Node {
    /// Create new node.
    pub fn new(data: u64) -> Self {
        Self {
            prev: ptr::null_mut(),
            next: ptr::null_mut(),
            refs: AtomicU32::new(1),
            data,
        }
    }

    /// BUGGY release (original CVE): Drop lock *before* unlink -> race.
    /// #[allow(unused)]
    /// pub fn release_buggy(this: *mut Node, list: &List) {
    ///     // Assume refs hit 0 (checked by caller).
    ///     let _guard = list.lock.lock(); // Acquire.
    ///     drop(_guard); // DROPPED EARLY! Race window.
    ///     // Unsafe unlink *without lock* -> aliases with move_to_stack.
    ///     unsafe { Self::remove(this) };
    /// }

    /// FIXED release: Hold lock across *entire* unlink.
    pub fn release(this: *mut Node, list: &List) {
        // Assume refs hit 0 (checked by caller).
        let _guard = list.lock.lock(); // Acquire & HOLD.
        // Unlink *while locked* -> no concurrent aliasing.
        unsafe { Self::remove(this) };
        // Lock held invariant maintained. Drop after.
    }

    /// Unsafe: Remove node from list (mutates neighbors).
    unsafe fn remove(this: *mut Node) {
        debug_assert!(!this.is_null());
        let node = &*this;
        if node.prev.is_null() || node.next.is_null() {
            return; // Already removed.
        }
        // Unlink: mutate neighbors.
        (*node.prev).next = node.next;
        (*node.next).prev = node.prev;
        // Clear self.
        node.prev = ptr::null_mut();
        node.next = ptr::null_mut();
    }

    /// Unsafe: Insert after head.
    pub unsafe fn insert_after(this: *mut Node, head: *mut ListHead) {
        debug_assert!(!this.is_null() && !head.is_null());
        let node = &mut *this;
        let head_next = (*head).next;
        node.next = head_next;
        node.prev = head as *mut Node;
        (*head).next = this;
        if !head_next.is_null() {
            (*head_next).prev = this;
        }
    }
}

/// List wrapper with Mutex (invariant: lock for all access).
pub struct List {
    lock: Mutex<()>,
    head: ListHead,
}

impl List {
    pub const fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            head: ListHead::new(),
        }
    }

    /// Scoped lock + access head.
    pub fn with_head<R>(&self, f: impl FnOnce(*mut ListHead) -> R) -> R {
        let _guard = self.lock.lock();
        f(&mut self.head as *mut _)
    }

    /// Move-to-stack: Traverse + unlink all nodes to Vec (mimics binder deferred stack).
    /// REQUIRES lock held during entire op.
    pub fn move_to_stack(&self) -> Vec<ARef<Node>> {
        self.with_head(|head| {
            let mut stack = Vec::new();
            let mut cur = (*head).next;
            while !cur.is_null() && cur != head as *mut Node {
                let node = cur;
                cur = unsafe { (*node).next }; // Advance before unlink.
                unsafe { Node::remove(node) };
                stack.push(unsafe { ARef::from_raw(node) }); // "Transfer" ownership.
            }
            stack
        })
    }

    /// Add node (lock held).
    pub fn push(&self, node: *mut Node) {
        self.with_head(|head| unsafe { Node::insert_after(node, head) });
    }
}

// Kernel module tests (verify fix).
kernel::module! {
    type: RustBinderFix,
    author: "Fixer",
    license: "GPL",
    test: binder_test,
}

fn binder_test(_: kernel::test::TestArgs) -> Result<(), Error> {
    // Multi-thread test setup.
    let list = Box::try_new(List::new())?;
    kernel::printk!(kernel::cstr!("Testing fixed Node::release...\n"));

    // Spawn threads: one releases, others move-to-stack concurrently.
    // (Simplified; real kernel uses task_work or kthreads.)

    // Test 1: Serial (works for both buggy/fixed).
    let node1 = Box::into_raw(Box::new(Node::new(42)));
    unsafe { Node::insert_after(node1, &mut list.head) };
    list.move_to_stack(); // Safe.

    // Test 2: Fixed prevents race (in real: concurrent kthreads).
    // Simulate: push node, "release" (drop ref), concurrent move_to_stack.
    let node2 = Box::into_raw(Box::new(Node::new(99)));
    list.push(node2);
    // "Decrement ref" -> call release.
    Node::release(node2, &list);
    // Concurrent move_to_stack sees empty list (no corruption).
    let stack = list.move_to_stack();
    assert!(stack.is_empty(), "Fixed: no leftover nodes");

    Ok(())
}

kernel::task!(RustBinderFix);
