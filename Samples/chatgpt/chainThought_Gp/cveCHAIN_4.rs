use std::sync::{Arc, Mutex};
use std::ptr;

// Define a node in the intrusive list
struct ListNode<T> {
    value: T,
    next: Option<*mut ListNode<T>>,
    prev: Option<*mut ListNode<T>>,
}

// Define the intrusive list
struct IntrusiveList<T> {
    head: Option<*mut ListNode<T>>,
    tail: Option<*mut ListNode<T>>,
    lock: Mutex<()>, // Mutex to protect the list
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList {
            head: None,
            tail: None,
            lock: Mutex::new(()),
        }
    }

    // Add a node to the list
    fn push_front(&mut self, node: &mut ListNode<T>) {
        let _guard = self.lock.lock().unwrap(); // Acquire the lock
        unsafe {
            node.next = self.head;
            node.prev = None;
            if let Some(head) = self.head {
                (*head).prev = Some(node);
            }
            self.head = Some(node);
            if self.tail.is_none() {
                self.tail = self.head;
            }
        }
    }

    // Remove a node from the list
    fn remove(&mut self, node: &mut ListNode<T>) {
        let _guard = self.lock.lock().unwrap(); // Acquire the lock
        unsafe {
            if let Some(prev) = node.prev {
                (*prev).next = node.next;
            } else {
                self.head = node.next;
            }

            if let Some(next) = node.next {
                (*next).prev = node.prev;
            } else {
                self.tail = node.prev;
            }

            node.next = None;
            node.prev = None;
        }
    }

    // Move a node to another context (e.g., stack)
    fn move_to_stack(&mut self, node: &mut ListNode<T>) -> Option<T> {
        let _guard = self.lock.lock().unwrap(); // Acquire the lock
        self.remove(node);

        // Assuming node value is wrapped in an Option, take the value
        Some(unsafe { ptr::read(&node.value) })
    }
}

fn main() {
    let mut list: IntrusiveList<i32> = IntrusiveList::new();
    let mut node1 = ListNode { value: 1, next: None, prev: None };
    let mut node2 = ListNode { value: 2, next: None, prev: None };

    list.push_front(&mut node1);
    list.push_front(&mut node2);

    if let Some(value) = list.move_to_stack(&mut node1) {
        println!("Moved value to stack: {}", value);
    }
}