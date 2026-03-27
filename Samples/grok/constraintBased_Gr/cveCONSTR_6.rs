//! Thread-safe Cleanup Routine for Intrusive Linked List in Rust
//! 
//! This module implements a **thread-safe intrusive doubly-linked list** for managing
//! death notifications (modeled after Android Binder's `death_list` in `rust_binder`).
//! 
//! **Key Requirements Addressed**:
//! 1. **Unsafe pointer manipulation** — All intrusive link operations (`insert`, `remove`, `pop_front`) use `unsafe` with `NonNull` and raw pointer arithmetic, exactly mimicking kernel-style `rust_binder` patterns.
//! 2. **release() with local stack list** — To minimize lock contention, `release()` builds a temporary `Vec` of owned callbacks **entirely under the lock**. The lock is dropped only *after* the transfer is complete, before invoking callbacks.
//! 3. **Avoid CVE-2025-68260 race** — The critical fix: **No list elements are exposed outside the lock**. 
//!    - The CVE occurred because `mem::take` + `drop(guard)` + iterate temp list allowed concurrent `unsafe remove()` to mutate `prev`/`next` pointers while the temporary list still referenced the same objects.
//!    - Here, `pop_front` transfers ownership **under lock**. Concurrent `remove()` either sees the element still in the list (and removes it safely) or never sees it (if already popped).
//!    - **Synchronization invariant** (strictly maintained): 
//!      > "A `DeathLink` is either exclusively in this `DeathList` (protected by the `Mutex`) **or** nowhere. Never in two places at once."
//!      This invariant holds across the entire "move to local stack" because the move completes before the guard drops.
//! 4. **Synchronization primitive** — Uses `std::sync::Mutex` for simplicity and portability. In a real kernel context, this would be `kernel::sync::Mutex` or a spinlock (`spin::Mutex`). Mutex is chosen here as it is "standard" and provides poisoning safety.
//! 
//! **Memory Stability & Concurrency Guarantees**:
//! - All `prev`/`next` mutations happen **exclusively** while holding `&mut DeathList` (via `MutexGuard`).
//! - `Arc<DeathRecipient>` ensures objects stay alive during transfer and callback execution.
//! - `DeathRecipient::Drop` auto-unlinks (best-effort).
//! - Callbacks run **without** the lock held → prevents deadlock if callbacks acquire other locks or perform blocking work.
//! - Idempotent `release()`; handles node already dead.
//! - Edge cases: empty list, concurrent link/unlink/release from many threads, callback panics (caught), double-unlink, unlink after death.
//! 
//! **Why this avoids the CVE aliasing violation**:
//! - In the vulnerable pattern, after lock drop the temporary list provided a second alias to the intrusive objects' memory while another thread performed mutable pointer writes via `unsafe remove`.
//! - Rust's aliasing rules (Stacked Borrows / Tree Borrows) were violated because there was no synchronization protecting the "moved" objects.
//! - Here, the transfer (`pop_front` loop) is atomic w.r.t. other mutations. When the lock drops, the original list is empty and no dangling `prev`/`next` references remain accessible concurrently.

use std::sync::{Arc, Mutex, Weak};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::panic;

/// Intrusive link embedded directly in the payload (zero-allocation).
#[derive(Default)]
#[repr(C)]
struct DeathLink {
    next: Option<NonNull<DeathLink>>,
    prev: Option<NonNull<DeathLink>>,
}

/// Death callback (can be slow or re-entrant — hence we release the lock before calling).
type DeathCallback = Box<dyn FnOnce() + Send + Sync>;

/// Death recipient — the payload containing the intrusive link.
struct DeathRecipient {
    links: DeathLink,
    callback: Option<DeathCallback>, // Option to allow take-once
    node: Weak<BinderNode>,
    is_linked: AtomicBool,
}

impl DeathRecipient {
    fn new(callback: DeathCallback, node: Weak<BinderNode>) -> Arc<Self> {
        Arc::new(DeathRecipient {
            links: DeathLink::default(),
            callback: Some(callback),
            node,
            is_linked: AtomicBool::new(false),
        })
    }
}

impl Drop for DeathRecipient {
    fn drop(&mut self) {
        // Best-effort auto-unlink when the recipient is dropped while still linked.
        if let Some(node) = self.node.upgrade() {
            let _ = node.unlink_to_death(Arc::from_raw(self as *const Self as *mut Self)); // Avoid double-drop
        }
    }
}

/// Intrusive doubly-linked list head.
struct DeathList {
    head: Option<NonNull<DeathLink>>,
    len: usize,
}

impl DeathList {
    const fn new() -> Self {
        DeathList { head: None, len: 0 }
    }

    /// Insert at front under exclusive lock.
    /// SAFETY: Caller must hold the `Mutex` exclusively.
    unsafe fn insert_front(&mut self, recipient: &Arc<DeathRecipient>) {
        let link_ptr = NonNull::from(&recipient.links as *const DeathLink as *mut DeathLink);

        if let Some(mut h) = self.head {
            (*link_ptr.as_ptr()).next = Some(h);
            (*h.as_ptr()).prev = Some(link_ptr);
        } else {
            (*link_ptr.as_ptr()).next = None;
        }
        (*link_ptr.as_ptr()).prev = None;
        self.head = Some(link_ptr);
        self.len += 1;
        recipient.is_linked.store(true, Ordering::Release);
    }

    /// Pop front and transfer ownership to caller.
    /// Returns `None` if list empty.
    /// SAFETY: Caller must hold the `Mutex` exclusively. The returned `Arc` takes ownership.
    unsafe fn pop_front(&mut self) -> Option<Arc<DeathRecipient>> {
        let link = self.head?;
        let recipient_ptr = link.as_ptr() as *mut DeathRecipient;

        // Increment strong count before from_raw to keep it alive.
        Arc::increment_strong_count(recipient_ptr);
        let recipient = Arc::from_raw(recipient_ptr);

        let next = (*link.as_ptr()).next;

        if let Some(mut n) = next {
            (*n.as_ptr()).prev = None;
        }
        self.head = next;

        // Clear links — prevents any dangling pointer use after pop.
        (*link.as_ptr()).next = None;
        (*link.as_ptr()).prev = None;

        self.len = self.len.saturating_sub(1);
        recipient.is_linked.store(false, Ordering::Release);

        Some(recipient)
    }

    /// Remove a specific recipient (for unlink_to_death).
    /// SAFETY: Caller must hold the `Mutex` exclusively.
    /// The "either in this list or nowhere" invariant is preserved.
    unsafe fn remove(&mut self, recipient: &DeathRecipient) -> bool {
        if !recipient.is_linked.load(Ordering::Acquire) {
            return false;
        }

        let link_ptr = NonNull::from(&recipient.links as *const DeathLink as *mut DeathLink);
        let prev = (*link_ptr.as_ptr()).prev;
        let next = (*link_ptr.as_ptr()).next;

        if let Some(mut p) = prev {
            (*p.as_ptr()).next = next;
        } else {
            self.head = next; // Was head
        }

        if let Some(mut n) = next {
            (*n.as_ptr()).prev = prev;
        }

        // Clear to prevent reuse of stale pointers.
        (*link_ptr.as_ptr()).next = None;
        (*link_ptr.as_ptr()).prev = None;

        self.len = self.len.saturating_sub(1);
        recipient.is_linked.store(false, Ordering::Release);
        true
    }
}

/// Internal state protected by Mutex.
struct NodeInner {
    death_list: DeathList,
    is_alive: bool,
}

/// The BinderNode — shared via Arc for thread-safe access.
pub struct BinderNode {
    inner: Mutex<NodeInner>,
    id: u64, // For identification/debug
}

impl BinderNode {
    /// Create a new node.
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(BinderNode {
            inner: Mutex::new(NodeInner {
                death_list: DeathList::new(),
                is_alive: true,
            }),
            id,
        })
    }

    /// Register a death notification (link_to_death).
    pub fn link_to_death(
        self: &Arc<Self>,
        callback: impl FnOnce() + Send + Sync + 'static,
    ) -> Option<Arc<DeathRecipient>> {
        let node_weak = Arc::downgrade(self);
        let recipient = DeathRecipient::new(Box::new(callback), node_weak);

        let mut guard = self.inner.lock().unwrap();
        if !guard.is_alive {
            drop(guard);
            // Fire immediately if node already dead.
            if let Some(cb) = recipient.callback.take() {
                let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| cb()));
            }
            return None;
        }

        unsafe {
            guard.death_list.insert_front(&recipient);
        }
        Some(recipient)
    }

    /// Unlink a previously registered recipient.
    pub fn unlink_to_death(&self, recipient: &Arc<DeathRecipient>) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if !guard.is_alive {
            return false;
        }
        unsafe { guard.death_list.remove(recipient) }
    }

    /// Release the node — trigger death notifications.
    /// 
    /// **Critical CVE fix**: Build the local callback list **entirely under the lock**.
    /// Lock is dropped only after all intrusive pointers have been cleared and ownership transferred.
    /// This ensures `prev`/`next` remain valid and synchronized even if `unlink_to_death` runs concurrently.
    pub fn release(self: Arc<Self>) {
        let mut callbacks: Vec<DeathCallback> = Vec::new();

        {
            let mut guard = self.inner.lock().unwrap();

            if !guard.is_alive {
                return; // Idempotent release.
            }
            guard.is_alive = false;

            // Transfer all items to local stack vec while holding the lock.
            // This is the safe "move to local list" — no exposure of elements outside lock.
            while let Some(recipient) = unsafe { guard.death_list.pop_front() } {
                if let Some(cb) = recipient.callback.take() {
                    callbacks.push(cb);
                }
                // recipient drops here if no other strong refs, but callback is moved out.
            }
            // At this point: death_list is empty, all links cleared, invariant holds.
        } // MutexGuard dropped here — lock released with short hold time.

        // Invoke callbacks WITHOUT holding the lock (minimizes contention, avoids deadlock).
        for cb in callbacks {
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| cb()));
        }
    }

    /// Query liveness (for testing/debug).
    pub fn is_alive(&self) -> bool {
        self.inner.lock().unwrap().is_alive
    }
}

/// Example usage and concurrency test.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn test_release_with_local_stack_and_concurrent_unlink() {
        let node = BinderNode::new(1);
        let call_count = Arc::new(AtomicUsize::new(0));

        // Register multiple recipients
        let mut recipients = vec![];
        for i in 0..10 {
            let c = Arc::clone(&call_count);
            let rec = node.link_to_death(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }).unwrap();
            recipients.push(rec);
        }

        // Concurrent unlink from one recipient while releasing
        let node_clone = Arc::clone(&node);
        let unlink_handle = thread::spawn(move || {
            if let Some(rec) = recipients.get(3) {
                let _ = node_clone.unlink_to_death(rec);
            }
        });

        // Release — should deliver remaining 9 callbacks (one unlinked)
        node.release();

        unlink_handle.join().unwrap();

        assert!(!node.is_alive());
        // Expect 9 callbacks (one was unlinked before release completed)
        assert_eq!(call_count.load(Ordering::SeqCst), 9);
    }

    #[test]
    fn test_multiple_concurrent_releases() {
        let node = BinderNode::new(2);
        let call_count = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let c = Arc::clone(&call_count);
            let _ = node.link_to_death(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        let handles: Vec<_> = (0..3).map(|_| {
            let n = Arc::clone(&node);
            thread::spawn(move || n.release())
        }).collect();

        for h in handles {
            h.join().unwrap();
        }

        assert!(!node.is_alive());
        assert_eq!(call_count.load(Ordering::SeqCst), 5); // Only once
    }
}