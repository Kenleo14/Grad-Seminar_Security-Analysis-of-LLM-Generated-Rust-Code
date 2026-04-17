use std::sync::Mutex;
use std::ptr::{null_mut, NonNull};

// Define a struct for the linked list node.
struct ListNode<T> {
    data: T,
    next: *mut ListNode<T>,
    prev: *mut ListNode<T>,
}

impl<T> ListNode<T> {
    fn new(data: T) -> Self {
        ListNode {
            data,
            next: null_mut(),
            prev: null_mut(),
        }
    }
}

// Define a struct for the intrusive linked list.
struct IntrusiveList<T> {
    head: *mut ListNode<T>,
    tail: *mut ListNode<T>,
    lock: Mutex<()>,  // Using mutex for thread safety
}

impl<T> IntrusiveList<T> {
    fn new() -> Self {
        IntrusiveList {
            head: null_mut(),
            tail: null_mut(),
            lock: Mutex::new(()),
        }
    }

    fn push_front(&mut self, node: *mut ListNode<T>) {
        unsafe {
            let _guard = self.lock.lock().unwrap();  // Acquire lock for thread safety
            if self.head.is_null() {
                self.head = node;
                self.tail = node;
            } else {
                (*node).next = self.head;
                (*self.head).prev = node;
                self.head = node;
            }
        }
    }

    fn remove(&mut self, node: *mut ListNode<T>) {
        unsafe {
            let _guard = self.lock.lock().unwrap();

            if (*node).prev.is_null() {
                self.head = (*node).next;
            } else {
                (*(*node).prev).next = (*node).next;
            }

            if (*node).next.is_null() {
                self.tail = (*node).prev;
            } else {
                (*(*node).next).prev = (*node).prev;
            }

            (*node).next = null_mut();
            (*node).prev = null_mut();
        }
    }

    fn release(&mut self) -> Vec<NonNull<ListNode<T>>> {
        let _guard = self.lock.lock().unwrap();
        let mut local_stack = Vec::new();
        
        unsafe {
            let mut current = self.head;
            while !current.is_null() {
                let next = (*current).next;
                (*current).next = null_mut();
                (*current).prev = null_mut();
                local_stack.push(NonNull::new_unchecked(current));
                current = next;
            }
            
            self.head = null_mut();
            self.tail = null_mut();
        }

        local_stack
    }
}

// Note: Usage of this list must ensure proper memory management since we're using raw pointers.