use kernel::sync::{SpinLock, Arc};
use kernel::list::{List, ListItem};

// A simplified Node using intrusive list links
struct Node {
    links: ListItem<Node>,
    data: i32,
}

struct BinderContext {
    // The list of all active nodes
    nodes: SpinLock<List<Node>>,
}

impl Node {
    fn release(target_node: Arc<Self>, context: &BinderContext) {
        // 1. Acquire the lock BEFORE attempting any removal
        let mut guard = context.nodes.lock();

        // 2. Critical Section: Check if the node is still in the list
        // and remove it while the lock is held.
        if target_node.links.is_linked() {
            // SAFETY: We hold the lock that protects the list.
            // We remove it from the global list so no other thread
            // can find it or create a new reference to it.
            unsafe { guard.remove(&target_node) };
        }

        // 3. NOW it is safe to drop the lock.
        drop(guard);

        // 4. The node is now truly "local" to this thread.
        // Rust's RAII will handle the actual memory deallocation 
        // once the Arc reference count hits zero, safely.
    }
}