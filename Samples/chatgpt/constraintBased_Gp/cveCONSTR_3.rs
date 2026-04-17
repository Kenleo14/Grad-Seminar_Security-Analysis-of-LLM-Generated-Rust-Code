use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;

// Define a struct for the list node
struct ListNode {
    data: i32,
    next: *mut ListNode,
    prev: *mut ListNode,
}

// Define a struct for the list
struct IntrusiveList {
    head: *mut ListNode,
    tail: *mut ListNode,
    mutex: Mutex<()>,
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            mutex: Mutex::new(()),
        }
    }

    // Add a node to the list
    unsafe fn add(&mut self, node: *mut ListNode) {
        let _lock = self.mutex.lock().unwrap();
        
        if self.head.is_null() {
            self.head = node;
            self.tail = node;
        } else {
            (*node).prev = self.tail;
            (*self.tail).next = node;
            self.tail = node;
        }
    }

    // Remove a node from the list
    unsafe fn remove(&mut self, node: *mut ListNode) {
        let _lock = self.mutex.lock().unwrap();

        if node.is_null() {
            return;
        }

        if node == self.head {
            self.head = (*node).next;
        }
        
        if node == self.tail {
            self.tail = (*node).prev;
        }

        if !(*node).prev.is_null() {
            (*(*node).prev).next = (*node).next;
        }
        
        if !(*node).next.is_null() {
            (*(*node).next).prev = (*node).prev;
        }
    }

    // Release all nodes to a local stack
    unsafe fn release(&mut self) -> Vec<*mut ListNode> {
        let _lock = self.mutex.lock().unwrap();

        let mut stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            stack.push(current);
            let next = (*current).next;
            (*current).next = ptr::null_mut();
            (*current).prev = ptr::null_mut();
            current = next;
        }

        self.head = ptr::null_mut();
        self.tail = ptr::null_mut();

        stack
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveList::new()));

    // Example usage
    let node1 = Box::into_raw(Box::new(ListNode {
        data: 1,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));

    let node2 = Box::into_raw(Box::new(ListNode {
        data: 2,
        next: ptr::null_mut(),
        prev: ptr::null_mut(),
    }));

    {
        let mut list = list.lock().unwrap();
        unsafe {
            list.add(node1);
            list.add(node2);
        }
    }

    // Simulate concurrent removal and release
    let list_clone = Arc::clone(&list);
    let handle = thread::spawn(move || {
        let mut list = list_clone.lock().unwrap();
        unsafe {
            list.remove(node1);
        }
    });

    handle.join().unwrap();

    let release_handle = thread::spawn(move || {
        let mut list = list.lock().unwrap();
        unsafe {
            list.release();
        }
    });

    release_handle.join().unwrap();
}