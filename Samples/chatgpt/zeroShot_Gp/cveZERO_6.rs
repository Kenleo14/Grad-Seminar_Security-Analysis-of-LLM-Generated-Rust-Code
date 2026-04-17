use std::sync::{Arc, Mutex};
use std::ptr::{self, NonNull};
use std::marker::PhantomData;

// Define a struct for the DeathNotification which will be used in the linked list.
struct DeathNotification {
    callback: Box<dyn Fn() + Send + Sync>, // Callback to be executed
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
}

// An intrusive linked list to manage DeathNotifications.
struct DeathNotificationList {
    head: Option<NonNull<DeathNotification>>,
    tail: Option<NonNull<DeathNotification>>,
    _marker: PhantomData<Box<DeathNotification>>, // Marker to avoid drop-checks
}

impl DeathNotificationList {
    fn new() -> Self {
        Self {
            head: None,
            tail: None,
            _marker: PhantomData,
        }
    }

    // Add a new death notification to the list.
    fn add(&mut self, notification: Box<DeathNotification>) {
        unsafe {
            let mut boxed = Box::into_raw(notification);
            (*boxed).next = None;
            (*boxed).prev = self.tail;

            if let Some(tail) = self.tail {
                (*tail.as_ptr()).next = Some(NonNull::new_unchecked(boxed));
            } else {
                self.head = Some(NonNull::new_unchecked(boxed));
            }

            self.tail = Some(NonNull::new_unchecked(boxed));
        }
    }

    // Remove a death notification from the list.
    fn remove(&mut self, node: NonNull<DeathNotification>) {
        unsafe {
            let node_ref = node.as_ref();
            if let Some(prev) = node_ref.prev {
                prev.as_ref().next = node_ref.next;
            } else {
                self.head = node_ref.next;
            }

            if let Some(next) = node_ref.next {
                next.as_ref().prev = node_ref.prev;
            } else {
                self.tail = node_ref.prev;
            }
        }
    }

    // Move all nodes to a temporary list for cleanup.
    fn move_to_temp(&mut self) -> DeathNotificationList {
        let temp_list = DeathNotificationList {
            head: self.head.take(),
            tail: self.tail.take(),
            _marker: PhantomData,
        };
        temp_list
    }
}

struct BinderNode {
    death_notifications: Arc<Mutex<DeathNotificationList>>,
}

impl BinderNode {
    fn new() -> Self {
        Self {
            death_notifications: Arc::new(Mutex::new(DeathNotificationList::new())),
        }
    }

    fn add_death_notification(&self, callback: Box<dyn Fn() + Send + Sync>) {
        let notification = Box::new(DeathNotification {
            callback,
            next: None,
            prev: None,
        });
        let mut list = self.death_notifications.lock().unwrap();
        list.add(notification);
    }

    fn release(&self) {
        let mut list = self.death_notifications.lock().unwrap();
        let temp_list = list.move_to_temp();
        drop(list); // Explicitly drop the lock.

        // Cleanup operations can be done here with temp_list if needed
    }
}

fn main() {
    let node = BinderNode::new();
    
    node.add_death_notification(Box::new(|| println!("Node died!")));
    node.release();
}