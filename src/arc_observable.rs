use core::result::Result as CoreResult;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use lum_libs::parking_lot::Mutex;

use crate::{Event, subscriber::DispatchError};

#[derive(Debug)]
pub enum Result<T> {
    Unchanged,
    Changed(CoreResult<(), Vec<DispatchError<Arc<Mutex<T>>>>>),
}

#[derive(Debug)]
pub struct ArcObservable<T: Send + Hash> {
    pub on_change: Event<Arc<Mutex<T>>>,

    value: Arc<Mutex<T>>,
}

impl<T: Send + Hash> ArcObservable<T> {
    pub fn new(value: T, event_name: impl Into<String>) -> Self {
        Self {
            value: Arc::new(Mutex::new(value)),
            on_change: Event::new(event_name),
        }
    }

    pub fn get(&self) -> Arc<Mutex<T>> {
        self.value.clone()
    }

    pub async fn set(&self, value: T) -> Result<T> {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let new_value_hash = hasher.finish();

        {
            let mut current_value = self.value.lock();
            let mut hasher = DefaultHasher::new();
            current_value.hash(&mut hasher);
            let current_value_hash = hasher.finish();

            if current_value_hash == new_value_hash {
                return Result::Unchanged;
            }

            *current_value = value;
        }

        let value = self.value.clone();
        let dispatch_result = self.on_change.dispatch(value).await;
        match dispatch_result {
            Ok(_) => Result::Changed(Ok(())),
            Err(errors) => Result::Changed(Err(errors)),
        }
    }
}

impl<T: Send + Hash> AsRef<Event<Arc<Mutex<T>>>> for ArcObservable<T> {
    fn as_ref(&self) -> &Event<Arc<Mutex<T>>> {
        &self.on_change
    }
}

impl<T: Send + Hash> Hash for ArcObservable<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let value = self.value.lock();
        value.hash(state);
    }
}

impl<T: Send + Hash> PartialEq for ArcObservable<T> {
    fn eq(&self, other: &Self) -> bool {
        let mut hasher = DefaultHasher::new();
        self.value.lock().hash(&mut hasher);
        let self_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        other.value.lock().hash(&mut hasher);
        let other_hash = hasher.finish();

        self_hash == other_hash
    }
}

impl<T: Send + Hash> PartialEq<T> for ArcObservable<T> {
    fn eq(&self, other: &T) -> bool {
        let mut hasher = DefaultHasher::new();
        self.value.lock().hash(&mut hasher);
        let self_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        other.hash(&mut hasher);
        let other_hash = hasher.finish();

        self_hash == other_hash
    }
}

impl<T: Send + Hash> Eq for ArcObservable<T> {}
