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
    Changed(CoreResult<(), Vec<DispatchError<Arc<T>>>>),
}

#[derive(Debug)]
pub struct ArcObservable<T: Send + Sync + Hash> {
    pub on_change: Event<Arc<T>>,

    value: Mutex<Arc<T>>,
}

impl<T: Send + Sync + Hash> ArcObservable<T> {
    pub fn new(value: T, event_name: impl Into<String>) -> Self {
        Self {
            value: Mutex::new(Arc::new(value)),
            on_change: Event::new(event_name),
        }
    }

    pub fn get(&self) -> Arc<T> {
        self.value.lock().clone()
    }

    //TODO: Docs about cancelation safety. value can be dropped without reaching a channel.
    pub async fn set(&self, value: T) -> Result<T> {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let new_value_hash = hasher.finish();

        let set_value: Arc<T>;
        {
            let mut current_value = self.value.lock();
            let mut hasher = DefaultHasher::new();
            current_value.hash(&mut hasher);
            let current_value_hash = hasher.finish();

            if current_value_hash == new_value_hash {
                return Result::Unchanged;
            }

            set_value = Arc::new(value);
            *current_value = set_value.clone();
        }

        let dispatch_result = self.on_change.dispatch(set_value).await;
        match dispatch_result {
            Ok(_) => Result::Changed(Ok(())),
            Err(errors) => Result::Changed(Err(errors)),
        }
    }
}

impl<T: Send + Sync + Hash> AsRef<Event<Arc<T>>> for ArcObservable<T> {
    fn as_ref(&self) -> &Event<Arc<T>> {
        &self.on_change
    }
}

impl<T: Send + Sync + Hash> Hash for ArcObservable<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let value = self.value.lock();
        value.hash(state);
    }
}

impl<T: Send + Sync + Hash> PartialEq for ArcObservable<T> {
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

impl<T: Send + Sync + Hash> PartialEq<T> for ArcObservable<T> {
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

impl<T: Send + Sync + Hash> Eq for ArcObservable<T> {}
