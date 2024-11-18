use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use lum_libs::tokio::sync::Mutex;

use crate::{subscriber::DispatchError, Event};

#[derive(Debug)]
pub enum Result<T> {
    Unchanged,
    Changed(core::result::Result<(), Vec<DispatchError<Arc<Mutex<T>>>>>),
}

#[derive(Debug)]
pub struct ArcObservable<T>
where
    T: Send + Hash,
{
    pub on_change: Event<Arc<Mutex<T>>>,

    value: Arc<Mutex<T>>,
}

impl<T> ArcObservable<T>
where
    T: Send + Hash,
{
    pub fn new<STRING>(value: T, event_name: STRING) -> Self
    where
        STRING: Into<String>,
    {
        Self {
            value: Arc::new(Mutex::new(value)),
            on_change: Event::new(event_name),
        }
    }

    pub fn get(&self) -> Arc<Mutex<T>> {
        self.value.clone()
    }

    pub async fn set(&mut self, value: T) -> Result<T> {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let new_value_hash = hasher.finish();

        {
            let mut current_value = self.value.lock().await;
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

impl<T> AsRef<Event<Arc<Mutex<T>>>> for ArcObservable<T>
where
    T: Send + Hash,
{
    fn as_ref(&self) -> &Event<Arc<Mutex<T>>> {
        &self.on_change
    }
}

impl<T> AsMut<Event<Arc<Mutex<T>>>> for ArcObservable<T>
where
    T: Send + Hash,
{
    fn as_mut(&mut self) -> &mut Event<Arc<Mutex<T>>> {
        &mut self.on_change
    }
}

impl<T> Hash for ArcObservable<T>
where
    T: Send + Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.blocking_lock().hash(state);
    }
}

impl<T> PartialEq for ArcObservable<T>
where
    T: Send + Hash,
{
    fn eq(&self, other: &Self) -> bool {
        let mut hasher = DefaultHasher::new();
        self.value.blocking_lock().hash(&mut hasher);
        let self_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        other.value.blocking_lock().hash(&mut hasher);
        let other_hash = hasher.finish();

        self_hash == other_hash
    }
}

impl<T> PartialEq<T> for ArcObservable<T>
where
    T: Send + Hash,
{
    fn eq(&self, other: &T) -> bool {
        let mut hasher = DefaultHasher::new();
        self.value.blocking_lock().hash(&mut hasher);
        let self_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        other.hash(&mut hasher);
        let other_hash = hasher.finish();

        self_hash == other_hash
    }
}

impl<T> Eq for ArcObservable<T> where T: Send + Hash {}
