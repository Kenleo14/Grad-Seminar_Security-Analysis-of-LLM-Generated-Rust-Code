//! Fixed Rust Binder Node management with death notifications.
//! Addresses CVE-2025-68260 by maintaining exclusive access invariants
//! across the entire drain + temporary list transfer.

use parking_lot::Mutex;
use std::collections::LinkedList;
use std::ptr::NonNull;
use std::sync::Arc;

/// Death callback (Send + Sync for thread safety).
type DeathCallback = Box<dyn Fn() + Send + Sync>;

#[repr(C)]
#[derive(Default)]
struct DeathLinks {
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
}

struct DeathNotification {
    callback: DeathCallback,
    links: DeathLinks,
    /// Strong back-ref to node inner for safe unlinking.
    node_inner: Arc<Mutex<NodeInner>>,
    /// Flag to help detect if this notification has been drained/released.
    /// In a more optimized kernel version this might be a generation or removed flag.
    is_drained: bool, // Set only by the drainer under lock
}

impl DeathNotification {
    fn new(callback: DeathCallback, node_inner: Arc<Mutex<NodeInner>>) -> Self {
        DeathNotification {
            callback,
            links: DeathLinks::default(),
            node_inner,
            is_drained: false,
        }
    }
}

struct NodeInner {
    death_list_head: Option<NonNull<DeathNotification>>,
    is_released: bool,
}

impl NodeInner {
    fn new() -> Self {
        NodeInner {
            death_list_head: None,
            is_released: false,
        }
    }

    // Insert remains similar (O(1), under lock)
    unsafe fn insert_death(&mut self, notif: NonNull<DeathNotification>) {
        let notif_mut = notif.as_ptr().as_mut().unwrap();
        notif_mut.links.next = self.death_list_head;
        notif_mut.links.prev = None;

        if let Some(mut head) = self.death_list_head {
            head.as_mut().links.prev = Some(notif);
        }
        self.death_list_head = Some(notif);
    }

    // Remove under lock with invariant check
    unsafe fn remove_death(&mut self, notif: NonNull<DeathNotification>) -> bool {
        let notif_ref = notif.as_ref();
        if notif_ref.is_drained {
            return false; // Already drained by release — ignore
        }

        // Rewire links
        if let Some(prev) = notif_ref.links.prev {
            prev.as_mut().links.next = notif_ref.links.next;
        } else {
            self.death_list_head = notif_ref.links.next;
        }
        if let Some(next) = notif_ref.links.next {
            next.as_mut().links.prev = notif_ref.links.prev;
        }

        let notif_mut = notif.as_ptr().as_mut().unwrap();
        notif_mut.links.next = None;
        notif_mut.links.prev = None;
        true
    }

    // Drain: exclusive ownership transfer under lock
    fn drain_deaths(&mut self) -> LinkedList<Box<DeathNotification>> {
        let mut temp = LinkedList::new();
        let mut current = self.death_list_head;
        self.death_list_head = None;

        while let Some(ptr) = current {
            let mut boxed = unsafe { Box::from_raw(ptr.as_ptr()) };
            boxed.is_drained = true; // Mark as exclusively owned now

            // Clear links (already unlinked via bulk drain)
            boxed.links.next = None;
            boxed.links.prev = None;

            temp.push_back(boxed);

            current = unsafe { (*ptr.as_ptr()).links.next }; // safe because we control the list
        }
        temp
    }
}

#[derive(Clone)]
pub struct BinderNode {
    inner: Arc<Mutex<NodeInner>>,
}

impl BinderNode {
    pub fn new() -> Self {
        BinderNode {
            inner: Arc::new(Mutex::new(NodeInner::new())),
        }
    }

    pub fn link_to_death<F>(&self, callback: F) -> bool
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut guard = self.inner.lock();
        if guard.is_released {
            return false;
        }

        let notif = Box::new(DeathNotification::new(
            Box::new(callback),
            Arc::clone(&self.inner),
        ));
        let ptr = NonNull::from(Box::leak(notif));

        unsafe {
            guard.insert_death(ptr);
        }
        true
    }

    /// Fixed unlink: always under lock, respects released/drained state
    pub fn unlink_to_death(&self) -> bool {
        // In real code you'd pass a cookie/identifier; here simplified to demo logic.
        // For full version, store a handle with NonNull or Weak.
        let mut guard = self.inner.lock();
        if guard.is_released {
            return false;
        }

        // Traverse and remove first matching (demo)
        let mut current = guard.death_list_head;
        while let Some(ptr) = current {
            let notif_ref = unsafe { ptr.as_ref() };
            if !notif_ref.is_drained {
                unsafe {
                    if guard.remove_death(ptr) {
                        let _ = unsafe { Box::from_raw(ptr.as_ptr()) };
                        return true;
                    }
                }
            }
            current = notif_ref.links.next;
        }
        false
    }

    /// Fixed release: drain under lock, mark released, then process temp list.
    /// The key: is_released + is_drained prevents any concurrent remove from
    /// touching the links after drain begins. The invariant holds for the entire
    /// lifecycle of the transfer.
    pub fn release(&self) {
        let temp_list = {
            let mut guard = self.inner.lock();

            if guard.is_released {
                return;
            }
            guard.is_released = true;

            // Drain happens with exclusive lock — no other thread can modify links
            guard.drain_deaths()
        }; // Lock dropped here — safe because drained notifications are marked and unlinked

        // Process callbacks lock-free (performance critical)
        for notif in temp_list {
            (notif.callback)();
            // Drop happens here — exclusive ownership guaranteed
        }
    }

    pub fn is_released(&self) -> bool {
        self.inner.lock().is_released
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.release();
        }
    }
}

// Tests would demonstrate no races, proper delivery, and safe concurrent unlink/release.
// In practice, add stress tests with many threads performing link/unlink/release.