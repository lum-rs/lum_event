use core::result::Result as CoreResult;
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use crate::{Event, subscriber::DispatchError};

#[derive(Debug)]
pub enum Result<T> {
    Unchanged,
    Changed(CoreResult<(), Vec<DispatchError<T>>>),
}

#[derive(Debug)]
pub struct Observable<T: Clone + Send + PartialEq> {
    pub on_change: Arc<Event<T>>,

    value: T,
}

impl<T: Clone + Send + PartialEq> Observable<T> {
    pub fn new(value: T, event_name: impl Into<String>) -> Self {
        Self {
            value,
            on_change: Arc::new(Event::new(event_name)),
        }
    }

    pub fn get(&self) -> T {
        self.value.clone()
    }

    //TODO: Docs about cancelation safety. value can be dropped without reaching a channel.
    pub async fn set(&mut self, value: T) -> Result<T> {
        if self.value == value {
            return Result::Unchanged;
        }

        self.value = value.clone();
        let dispatch_result = self.on_change.dispatch(value).await;
        match dispatch_result {
            Ok(_) => Result::Changed(Ok(())),
            Err(errors) => Result::Changed(Err(errors)),
        }
    }
}

impl<T: Clone + Send + PartialEq> AsRef<T> for Observable<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl AsRef<str> for Observable<&str> {
    fn as_ref(&self) -> &str {
        self.value
    }
}

impl<T: Clone + Send + PartialEq> AsRef<Event<T>> for Observable<T> {
    fn as_ref(&self) -> &Event<T> {
        self.on_change.as_ref()
    }
}

impl<T: Clone + Send + PartialEq> PartialEq for Observable<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Clone + Send + PartialEq> PartialEq<T> for Observable<T> {
    fn eq(&self, other: &T) -> bool {
        self.value == *other
    }
}

impl<T: Clone + Send + PartialEq> Eq for Observable<T> {}

impl<T: Clone + Send + PartialEq + Hash> Hash for Observable<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}
