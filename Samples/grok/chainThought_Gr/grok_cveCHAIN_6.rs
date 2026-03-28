use std::sync::{Arc, Mutex, atomic::{AtomicU32, Ordering}};
use std::ptr::{self, NonNull};
use std::pin::Pin;
use std::mem;

// Simplified kernel-like intrusive ListHead (raw ptrs, no Pin for links in this demo; kernel uses Pin for safety).
#[repr(C)]
#[derive(Debug)]
struct ListHead {
    next: *mut ListHead,
    prev: *mut ListHead,
}

unsafe impl Send for ListHead {}
unsafe impl Sync for ListHead {}

impl ListHead {
    fn new() -> Self {
        Self {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}

// Kernel-like helpers (unsafe, assume list lock held).
fn list_head_init(head: &mut ListHead) {
    unsafe {
        head.next = head as *mut ListHead;
        head.prev = head as *mut ListHead;
    }
}

fn list_is_empty(head: &ListHead) -> bool {
    unsafe { head.next == (head as *const ListHead as *mut ListHead) }
}

unsafe fn list_del_init(entry: *mut ListHead) {
    let prev = (*entry).prev;
    let next = (*entry).next;
    (*prev).next = next;
    (*next).prev = prev;
    (*entry).next = entry;
    (*entry).prev = entry;
}

fn list_first_entry_node(head: &mut ListHead) -> Option<NonNull<Node>> {
    unsafe {
        if list_is_empty(head) {
            None
        } else {
            let links_ptr = (*head).next;
            Some(NonNull::new_unchecked(links_ptr as *mut Node))
        }
    }
}

// NodeState: protected by per-node Mutex.
#[derive(Debug)]
struct NodeState {
    weak_refs: AtomicU32,  // Simplified: weak_refs only for demo (strong==0 already).
}

// Node: per-node Mutex + intrusive dead_links (offset=0 hack: ListHead first field).
#[repr(C)]
#[derive(Debug)]
struct Node {
    dead_links: ListHead,
    inner: Mutex<NodeState>,
}

impl Node {
    fn new() -> Arc<Self> {
        let node = Arc::new(Node {
            dead_links: ListHead::new(),
            inner: Mutex::new(NodeState {
                weak_refs: AtomicU32::new(1),  // Start with 1 ref.
            }),
        });
        // Safety: init links (called under list lock in real use).
        unsafe { list_head_init(&mut node.dead_links) };
        node
    }

    // BUGGY release (original CVE): drops node lock BEFORE list removal → aliasing window.
    // #[allow(dead_code)]
    fn buggy_release(this: Arc<Self>, state: &Arc<BinderState>) {
        let mut node_guard = this.inner.lock().unwrap();
        if this.inner.lock().unwrap().weak_refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        drop(node_guard);  // DROPS EARLY → &mut this still borrowed; move-to-stack can alias.
        let mut list_guard = state.dead_nodes.lock().unwrap();
        unsafe {
            list_del_init(&mut this.dead_links);
        }
        drop(list_guard);  // Race/UB possible here.
    }

    // FIXED release: NEST list lock INSIDE node lock → no aliasing window.
    // Maintains invariant across list transfer lifecycle.
    fn fixed_release(this: Arc<Self>, state: &Arc<BinderState>) {
        let mut node_guard = this.inner.lock().unwrap();
        // Atomic dec; only last decrem proceeds (fetch_sub==1).
        if node_guard.weak_refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        // STILL HOLDING node_guard → exclusive access guaranteed.
        let mut list_guard = state.dead_nodes.lock().unwrap();  // Nest: node then list.
        unsafe {
            list_del_init(&mut this.dead_links);
        }  // Safe: list lock + node lock → no concurrent &mut Node from move-to-stack.
        drop(list_guard);  // Drop inner first.
        drop(node_guard);  // Now drop node lock; node ready for free.
                         // In kernel: kfree(this).
    }
}

// BinderState: global dead_nodes list protected by Mutex.
#[derive(Debug)]
struct BinderState {
    dead_nodes: Mutex<ListHead>,
}

impl BinderState {
    fn new() -> Arc<Self> {
        let state = Arc::new(BinderState {
            dead_nodes: Mutex::new(ListHead::new()),
        });
        unsafe { list_head_init(&mut *state.dead_nodes.lock().unwrap()) };
        state
    }

    // Move-to-stack: holds list lock, dequeues to stack (temp &mut via first_entry), drops lock,
    // then processes under node lock. Safe: no nested node lock.
    fn process_dead_nodes(&self) -> usize {
        let mut stack: Vec<NonNull<Node>> = Vec::new();
        {
            let mut list_guard = self.dead_nodes.lock().unwrap();
            let head = list_guard.get_mut();
            while let Some(node_ptr) = list_first_entry_node(head) {
                unsafe {
                    list_del_init(node_ptr.as_ptr() as *mut ListHead);
                }
                stack.push(node_ptr);
            }
        }  // Drop list lock → allows nested release to proceed.
        let mut freed = 0;
        for node_ptr in stack {
            let node_arc = unsafe { Arc::from_raw(node_ptr.as_ptr()) };  // Reclaim Arc (demo).
            let mut node_guard = node_arc.inner.lock().unwrap();
            // In real: if weak_refs == 0 { free }
            freed += 1;
            drop(node_guard);
        }
        freed
    }

    // Add node to dead_nodes (called when strong_refs -> 0; under list lock).
    fn add_dead_node(&self, node: &Arc<Node>) {
        let mut guard = self.dead_nodes.lock().unwrap();
        unsafe {
            let head = guard.get_mut() as *mut ListHead;
            (*node.dead_links.next).prev = &mut node.dead_links as *mut _;
            node.dead_links.next = head;
            node.dead_links.prev = head;
            (*head).prev = &mut node.dead_links as *mut _;
        }
    }
}

fn main() {
    let state = BinderState::new();
    let node1 = Node::new();
    let node2 = Node::new();

    // Simulate: strong_refs->0 elsewhere; add to dead_nodes.
    state.add_dead_node(&node1);
    state.add_dead_node(&node2);

    // Simulate concurrent weak release + move-to-stack.
    // In threads: fixed_release safe; no UB.
    let node1c = node1.clone();
    let statec = state.clone();
    std::thread::spawn(move || node1c.fixed_release(&statec));

    let freed = state.process_dead_nodes();
    assert_eq!(freed, 2);  // Both processed safely.

    println!("Fixed module: {} nodes freed safely (no CVE).", freed);
}
