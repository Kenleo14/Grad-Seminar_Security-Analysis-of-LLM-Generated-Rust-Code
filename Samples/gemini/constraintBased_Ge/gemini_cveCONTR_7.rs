use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A simple Spinlock to mimic low-level kernel primitives (like rust_binder).
pub struct Spinlock {
    lock: AtomicBool,
}

impl Spinlock {
    pub const fn new() -> Self {
        Self { lock: AtomicBool::new(false) }
    }

    pub fn lock(&self) {
        while self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            std::hint::spin_loop();
        }
    }

    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

/// The Intrusive Node.
/// Data is stored alongside pointers.
pub struct Node<T> {
    pub data: T,
    pub next: Option<NonNull<Node<T>>>,
    pub prev: Option<NonNull<Node<T>>>,
    pub linked: bool,
}

pub struct IntrusiveList<T> {
    head: Option<NonNull<Node<T>>>,
    tail: Option<NonNull<Node<T>>>,
    lock: Spinlock,
}

impl<T> IntrusiveList<T> {
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
            lock: Spinlock::new(),
        }
    }

    /// Pushes a new node to the list.
    pub fn push(&mut self, data: T) {
        let node = Box::new(Node {
            data,
            next: None,
            prev: None,
            linked: true,
        });
        let node_ptr = NonNull::new(Box::into_raw(node));

        self.lock.lock();
        unsafe {
            let mut n = node_ptr.unwrap();
            n.as_mut().next = self.head;
            if let Some(mut old_head) = self.head {
                old_head.as_mut().prev = node_ptr;
            } else {
                self.tail = node_ptr;
            }
            self.head = node_ptr;
        }
        self.lock.unlock();
    }

    /// CVE-2025-68260 Avoidance:
    /// We minimize lock time by moving the entire list to a local "stack" list.
    /// The critical step is clearing the `linked` status and pointers while LOCKED.
    pub fn release(&mut self) {
        let mut local_head: Option<NonNull<Node<T>>> = None;

        // --- CRITICAL SECTION ---
        self.lock.lock();
        if let Some(head_ptr) = self.head {
            local_head = Some(head_ptr);
            
            // Traverse and mark nodes as unlinked so concurrent remove() calls
            // fail safely instead of touching invalid memory.
            let mut curr = local_head;
            while let Some(mut node_ptr) = curr {
                unsafe {
                    node_ptr.as_mut().linked = false;
                    curr = node_ptr.as_ref().next;
                }
            }

            // Sever the list from the global structure
            self.head = None;
            self.tail = None;
        }
        self.lock.unlock();
        // --- END CRITICAL SECTION ---

        // Cleanup happens here, outside the lock.
        let mut curr = local_head;
        while let Some(node_ptr) = curr {
            unsafe {
                // Re-taking ownership via Box ensures proper drop/deallocation
                let mut boxed_node = Box::from_raw(node_ptr.as_ptr());
                curr = boxed_node.next;
                
                // Clear pointers to prevent any dangling access during drop
                boxed_node.next = None;
                boxed_node.prev = None;
                
                drop(boxed_node);
            }
        }
    }

    /// Thread-safe individual removal.
    pub fn remove(&mut self, mut node_ptr: NonNull<Node<T>>) {
        self.lock.lock();
        unsafe {
            let node = node_ptr.as_mut();
            
            // Check if release() already claimed this node
            if !node.linked {
                self.lock.unlock();
                return;
            }

            if let Some(mut prev) = node.prev {
                prev.as_mut().next = node.next;
            } else {
                self.head = node.next;
            }

            if let Some(mut next) = node.next {
                next.as_mut().prev = node.prev;
            } else {
                self.tail = node.prev;
            }

            node.linked = false;
            let _ = Box::from_raw(node_ptr.as_ptr());
        }
        self.lock.unlock();
    }
}

// Cleanup on Drop
impl<T> Drop for IntrusiveList<T> {
    fn drop(&mut self) {
        self.release();
    }
}

fn main() {
    let mut list = IntrusiveList::new();
    list.push(10);
    list.push(20);
    list.push(30);

    println!("Cleaning up list...");
    list.release();
    println!("Done.");
}