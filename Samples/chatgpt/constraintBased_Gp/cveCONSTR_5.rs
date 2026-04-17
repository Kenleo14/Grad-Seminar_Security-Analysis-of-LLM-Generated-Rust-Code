use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;

struct Node {
    value: i32,
    next: *mut Node,
}

struct IntrusiveLinkedList {
    head: *mut Node,
    lock: Mutex<()>,
}

impl IntrusiveLinkedList {
    fn new() -> Self {
        IntrusiveLinkedList {
            head: ptr::null_mut(),
            lock: Mutex::new(()),
        }
    }

    // Add a new node to the list
    fn add(&mut self, value: i32) {
        let new_node = Box::into_raw(Box::new(Node { value, next: self.head }));
        self.head = new_node;
    }

    // Thread-safe release function that moves nodes to a local stack
    fn release(&mut self) -> Vec<i32> {
        let _guard = self.lock.lock().unwrap(); // Acquire lock

        let mut local_stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            unsafe {
                // Move value to the local stack
                local_stack.push((*current).value);

                // Move to the next node
                let next = (*current).next;

                // Deallocate the current node
                Box::from_raw(current);

                current = next;
            }
        }

        // Set head to null as list is now empty
        self.head = ptr::null_mut();

        local_stack
    }

    // Safe remove function to be used with release
    fn remove(&mut self, value: i32) -> bool {
        let _guard = self.lock.lock().unwrap(); // Acquire lock

        let mut current = self.head;
        let mut prev: *mut Node = ptr::null_mut();

        while !current.is_null() {
            unsafe {
                if (*current).value == value {
                    if !prev.is_null() {
                        (*prev).next = (*current).next;
                    } else {
                        self.head = (*current).next;
                    }

                    // Deallocate the removed node
                    Box::from_raw(current);

                    return true;
                }
                prev = current;
                current = (*current).next;
            }
        }

        false
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveLinkedList::new()));

    // Add some items to the list
    {
        let mut list = list.lock().unwrap();
        list.add(1);
        list.add(2);
        list.add(3);
    }

    // Spawn a thread to remove an item
    let list_clone = Arc::clone(&list);
    let remove_thread = thread::spawn(move || {
        let mut list = list_clone.lock().unwrap();
        list.remove(2);
    });

    // Release function called in the main thread
    let release_thread = thread::spawn(move || {
        let mut list = list.lock().unwrap();
        let local_stack = list.release();

        // Print the released items
        for item in local_stack {
            println!("Released item: {}", item);
        }
    });

    remove_thread.join().unwrap();
    release_thread.join().unwrap();
}