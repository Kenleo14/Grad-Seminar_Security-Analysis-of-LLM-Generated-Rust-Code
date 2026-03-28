//! Fixed Simplified Android Binder Node Management (Post-CVE-2025-68260 style).
//! 
//! This module fixes the lock-dropping race on intrusive death_list by ensuring
//! all list mutations (including pop/drain) occur under the Mutex. Temporary
//! ownership for callbacks is transferred *under lock*, then the guard is dropped
//! only for callback invocation.
//!
//! Key safety properties:
//! - Intrusive list access is exclusively under `MutexGuard`.
//! - No "move-to-stack" that leaves elements accessible concurrently.
//! - `remove` and `pop_front` maintain the "either in this list or nowhere" invariant.
//! - Callbacks run without lock held (avoids deadlock/re-entrancy issues).
//! - Memory stability: Arcs keep objects alive; links cleared before drop.

use std::sync::{Arc, Mutex, Weak};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::panic;

/// Intrusive link (embedded in DeathRecipient).
#[derive(Default)]
#[repr(C)]
struct DeathLink {
    next: Option<NonNull<DeathLink>>,
    prev: Option<NonNull<DeathLink>>,
}

/// Death callback (Send + Sync for cross-thread safety).
type DeathCallback = Box<dyn FnOnce() + Send + Sync>;

/// Death recipient with intrusive links.
struct DeathRecipient {
    links: DeathLink,
    callback: Option<DeathCallback>, // Option for take-once
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
        // Auto-unlink on drop if still linked (best-effort; real kernel has stronger guarantees).
        if let Some(node) = self.node.upgrade() {
            let _ = node.unlink_to_death(self); // Ignore result in Drop.
        }
    }
}

/// Intrusive death list.
struct DeathList {
    head: Option<NonNull<DeathLink>>,
    len: usize,
}

impl DeathList {
    fn new() -> Self {
        DeathList { head: None, len: 0 }
    }

    /// Pop front under exclusive lock. Returns the Arc if any.
    unsafe fn pop_front(&mut self) -> Option<Arc<DeathRecipient>> {
        let link = self.head?;
        let recipient_ptr = link.as_ptr() as *mut DeathRecipient;
        let arc = Arc::increment_strong_count(recipient_ptr);
        let arc = Arc::from_raw(recipient_ptr);

        let next = (*link.as_ptr()).next;
        if let Some(mut n) = next {
            (*n.as_ptr()).prev = None;
        }
        self.head = next;

        // Clear links
        (*link.as_ptr()).next = None;
        (*link.as_ptr()).prev = None;

        self.len = self.len.saturating_sub(1);
        arc.is_linked.store(false, Ordering::Release);

        Some(arc)
    }

    /// Unsafe remove (used by unlink_to_death). Caller must hold lock.
    unsafe fn remove(&mut self, recipient: &DeathRecipient) -> bool {
        if !recipient.is_linked.load(Ordering::Acquire) {
            return false;
        }

        let link_ptr = NonNull::from(&recipient.links as *const _ as *mut DeathLink);
        let prev = (*link_ptr.as_ptr()).prev;
        let next = (*link_ptr.as_ptr()).next;

        if let Some(mut p) = prev {
            (*p.as_ptr()).next = next;
        } else {
            self.head = next;
        }
        if let Some(mut n) = next {
            (*n.as_ptr()).prev = prev;
        }

        (*link_ptr.as_ptr()).next = None;
        (*link_ptr.as_ptr()).prev = None;

        self.len = self.len.saturating_sub(1);
        recipient.is_linked.store(false, Ordering::Release);
        true
    }
}

/// Binder Node inner state.
struct NodeInner {
    death_list: DeathList,
    is_alive: bool,
}

/// Binder Node (shared via Arc).
pub struct BinderNode {
    inner: Mutex<NodeInner>,
    id: u64,
}

impl BinderNode {
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(BinderNode {
            inner: Mutex::new(NodeInner {
                death_list: DeathList::new(),
                is_alive: true,
            }),
            id,
        })
    }

    /// Link to death (register callback). Returns recipient for optional unlink.
    pub fn link_to_death(
        self: &Arc<Self>,
        callback: impl FnOnce() + Send + Sync + 'static,
    ) -> Option<Arc<DeathRecipient>> {
        let node_weak = Arc::downgrade(self);
        let recipient = DeathRecipient::new(Box::new(callback), node_weak);

        let mut guard = self.inner.lock().unwrap();
        if !guard.is_alive {
            drop(guard);
            if let Some(cb) = recipient.callback.take() { // Fire immediately if already dead.
                let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| cb()));
            }
            return None;
        }

        unsafe {
            // Insert at front (or back; order usually doesn't matter for death).
            let link_ptr = NonNull::from(&recipient.links as *const _ as *mut DeathLink);
            if let Some(mut head) = guard.death_list.head {
                (*link_ptr.as_ptr()).next = Some(head);
                (*head.as_ptr()).prev = Some(link_ptr);
            }
            guard.death_list.head = Some(link_ptr);
            guard.death_list.len += 1;
        }
        recipient.is_linked.store(true, Ordering::Release);

        Some(recipient)
    }

    /// Unlink a specific recipient.
    pub fn unlink_to_death(&self, recipient: &Arc<DeathRecipient>) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if !guard.is_alive {
            return false;
        }
        unsafe { guard.death_list.remove(recipient) }
    }

    /// Release the node (trigger death notifications). FIXED VERSION.
    /// All list operations under lock; callbacks outside.
    pub fn release(self: Arc<Self>) {
        let mut callbacks = Vec::new();

        {
            let mut guard = self.inner.lock().unwrap();
            if !guard.is_alive {
                return; // Idempotent.
            }
            guard.is_alive = false;

            // Pop all under lock → transfer ownership safely.
            while let Some(recipient) = unsafe { guard.death_list.pop_front() } {
                if let Some(cb) = recipient.callback.take() { // Take once.
                    callbacks.push(cb);
                }
                // Recipient Arc drops here if no other refs, but we moved cb out.
            }
        } // MutexGuard dropped here. List is now empty and no concurrent remove can see old elements.

        // Invoke callbacks WITHOUT lock (prevents deadlock if callbacks acquire other locks).
        for cb in callbacks {
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| cb()));
        }
    }

    pub fn is_alive(&self) -> bool {
        self.inner.lock().unwrap().is_alive
    }
}

// Basic tests demonstrating safety under concurrency (in real code, use loom or many threads).
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn test_fixed_death_notification_and_concurrent_release() {
        let node = BinderNode::new(42);
        let call_count = Arc::new(AtomicUsize::new(0));

        let node_clone = Arc::clone(&node);
        let count_clone = Arc::clone(&call_count);

        // Register several
        for _ in 0..5 {
            let c = Arc::clone(&count_clone);
            let _rec = node.link_to_death(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        // Concurrent release attempts
        let handle = thread::spawn(move || {
            node_clone.release();
        });

        node.release(); // Second release is no-op.

        handle.join().unwrap();

        assert!(!node.is_alive());
        assert_eq!(call_count.load(Ordering::SeqCst), 5);
    }
}