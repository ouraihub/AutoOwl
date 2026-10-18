use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Resolving,
    Downloading,
    Processing,
    Completed,
    Failed,
    CancelRequested,
    Cancelled,
}

#[derive(Default)]
pub struct TaskRegistry {
    states: Mutex<HashMap<String, TaskState>>,
}

impl TaskRegistry {
    pub fn set(&self, id: String, state: TaskState) {
        if let Ok(mut states) = self.states.lock() {
            states.insert(id, state);
        }
    }

    pub fn get(&self, id: &str) -> Option<TaskState> {
        self.states
            .lock()
            .ok()
            .and_then(|states| states.get(id).copied())
    }
}
