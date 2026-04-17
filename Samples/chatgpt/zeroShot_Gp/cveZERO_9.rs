use std::sync::{Arc, Mutex, Condvar};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

struct DeathNotification {
    // Example fields
    id: usize,
    // Additional fields can be added here
}

struct BinderNode {
    notification: DeathNotification,
    next: AtomicPtr<BinderNode>,
}

impl BinderNode {
    fn new(notification: DeathNotification) -> Self {
        BinderNode {
            notification,
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

struct BinderNodeList {
    head: AtomicPtr<BinderNode>,
    lock: Mutex<()>,
    cleanup_cv: Condvar,
}

impl BinderNodeList {
    fn new() -> Self {
        BinderNodeList {
            head: AtomicPtr::new(ptr::null_mut()),
            lock: Mutex::new(()),
            cleanup_cv: Condvar::new(),
        }
    }

    fn add_node(&self, notification: DeathNotification) {
        let new_node = Box::into_raw(Box::new(BinderNode::new(notification)));
        let mut head = self.head.load(Ordering::Acquire);

        loop {
            unsafe { (*new_node).next.store(head, Ordering::Relaxed) };
            match self.head.compare_exchange_weak(
                head,
                new_node,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(h) => head = h,
            }
        }
    }

    fn release_nodes(&self) -> Vec<DeathNotification> {
        let _lock = self.lock.lock().unwrap();
        let mut nodes_to_cleanup = Vec::new();
        let mut current = self.head.swap(ptr::null_mut(), Ordering::Acquire);

        while !current.is_null() {
            unsafe {
                let node = Box::from_raw(current);
                nodes_to_cleanup.push(node.notification);
                current = node.next.load(Ordering::Relaxed);
            }
        }

        self.cleanup_cv.notify_all();
        nodes_to_cleanup
    }

    fn wait_for_cleanup(&self) {
        let lock = self.lock.lock().unwrap();
        self.cleanup_cv.wait(lock).unwrap();
    }
}

fn main() {
    // Example usage
    let node_list = Arc::new(BinderNodeList::new());

    let node_list_clone = node_list.clone();
    std::thread::spawn(move || {
        let notifications = node_list_clone.release_nodes();
        for notification in notifications {
            println!("Released notification with ID: {}", notification.id);
        }
    });

    for i in 0..10 {
        node_list.add_node(DeathNotification { id: i });
    }

    // Wait for cleanup to complete
    node_list.wait_for_cleanup();
}