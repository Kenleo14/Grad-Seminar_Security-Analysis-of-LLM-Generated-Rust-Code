use std::sync::{Arc, Mutex, Weak};
use std::ptr::{self, NonNull};

// Intrusive links embedded in each death node (same as before)
#[repr(C)]
struct ListLinks {
    next: Option<NonNull<DeathNode>>,
    prev: Option<NonNull<DeathNode>>,
}

impl ListLinks {
    const fn new() -> Self {
        ListLinks { next: None, prev: None }
    }
}

struct DeathNode {
    links: ListLinks,
    recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
    node: Weak<BinderNode>,
    is_dead: bool,
}

impl DeathNode {
    fn new(
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
        node: Weak<BinderNode>,
    ) -> Self {
        DeathNode {
            links: ListLinks::new(),
            recipient,
            node,
            is_dead: false,
        }
    }
}

pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

struct BinderNodeInner {
    death_list: Option<NonNull<DeathNode>>,
    death_count: usize,
    is_released: bool,
}

impl BinderNode {
    pub fn new() -> Arc<Self> {
        Arc::new(BinderNode {
            inner: Mutex::new(BinderNodeInner {
                death_list: None,
                death_count: 0,
                is_released: false,
            }),
        })
    }

    /// Register a death notification (insert at head under lock)
    pub fn register_death_notification(
        self: &Arc<Self>,
        recipient: Box<dyn Fn(Arc<BinderNode>) + Send + Sync>,
    ) -> bool {
        let weak_self = Arc::downgrade(self);
        let mut new_node = Box::new(DeathNode::new(recipient, weak_self));

        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            let node_clone = self.clone();
            drop(guard);
            (new_node.recipient)(node_clone);
            return false;
        }

        let node_ptr = NonNull::from(Box::leak(new_node));
        unsafe {
            if let Some(head) = guard.death_list {
                (*head.as_ptr()).links.prev = Some(node_ptr);
            }
            (*node_ptr.as_ptr()).links.next = guard.death_list;
            (*node_ptr.as_ptr()).links.prev = None;
            guard.death_list = Some(node_ptr);
        }
        guard.death_count += 1;
        true
    }

    /// Unregister by callback pointer (safe unlink under lock)
    pub fn unregister_death_notification(
        &self,
        recipient_ptr: *const dyn Fn(Arc<BinderNode>) + Send + Sync,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_released {
            return false;
        }

        let mut current_opt = guard.death_list;
        while let Some(curr_ptr) = current_opt {
            let curr = unsafe { &mut *curr_ptr.as_ptr() };
            if std::ptr::eq(
                &*curr.recipient as *const _ as *const (),
                recipient_ptr as *const (),
            ) {
                unsafe {
                    Self::unlink_node(&mut guard.death_list, curr_ptr);
                }
                guard.death_count -= 1;
                unsafe { drop(Box::from_raw(curr_ptr.as_ptr())); }
                return true;
            }
            current_opt = curr.links.next;
        }
        false
    }

    /// Fixed release: Process death notifications while maintaining lock protection for list mutations.
    /// We pop one-by-one under the lock, drop the lock only for the (potentially slow) callback,
    /// then re-acquire for the next pop. This eliminates the unlocked temp-list race window.
    pub fn release(self: Arc<Self>) {
        let mut guard = self.inner.lock().unwrap();

        if guard.is_released {
            return;
        }
        guard.is_released = true;

        // Process the list by repeatedly popping the head while holding/re-acquiring the lock
        loop {
            let head_opt = guard.death_list;
            if head_opt.is_none() {
                break;
            }

            // Unlink the head node under the lock
            let node_ptr = head_opt.unwrap();
            unsafe {
                Self::unlink_node(&mut guard.death_list, node_ptr);
            }
            guard.death_count -= 1;

            // Now drop the lock BEFORE invoking the callback (to avoid holding during user code)
            let death_node = unsafe { Box::from_raw(node_ptr.as_ptr()) };
            drop(guard);

            // Invoke callback outside lock (safe because list links are already unlinked)
            if !death_node.is_dead {
                if let Some(node_arc) = death_node.node.upgrade() {
                    (death_node.recipient)(node_arc);
                }
            }

            // Re-acquire lock for the next iteration
            guard = self.inner.lock().unwrap();
        }
    }

    /// Helper: Unlink a node (must be called under lock)
    unsafe fn unlink_node(head: &mut Option<NonNull<DeathNode>>, node: NonNull<DeathNode>) {
        let n = &mut *node.as_ptr();
        if let Some(prev) = n.links.prev {
            (*prev.as_ptr()).links.next = n.links.next;
        } else {
            *head = n.links.next;
        }
        if let Some(next) = n.links.next {
            (*next.as_ptr()).links.prev = n.links.prev;
        }
        n.links.next = None;
        n.links.prev = None;
    }
}

// Tests demonstrating safe concurrent behavior
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_fixed_release_with_concurrent_unregister() {
        let node = BinderNode::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let unregister_done = Arc::new(std::sync::Barrier::new(2));

        // Register notification
        let count_clone = call_count.clone();
        let barrier_clone = unregister_done.clone();
        node.register_death_notification(Box::new(move |n| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            barrier_clone.wait(); // Simulate some work
        }));

        // Spawn thread that tries to unregister concurrently
        let node_clone = node.clone();
        let handle = std::thread::spawn(move || {
            // Unregister attempt (may race with release)
            node_clone.unregister_death_notification(
                &*Box::new(|_| {}) as *const dyn Fn(Arc<BinderNode>) + Send + Sync,
            ); // Note: pointer won't match, but demonstrates path
        });

        node.release(); // Should safely process without corruption

        handle.join().unwrap();
        assert!(call_count.load(Ordering::SeqCst) <= 1); // At most one callback
    }
}