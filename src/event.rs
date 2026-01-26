use std::{
    any::type_name,
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::Deref,
    sync::{Arc, Weak},
};

use lum_boxtypes::{BoxedError, PinnedBoxedFutureResult};
use lum_libs::{
    dashmap::DashMap,
    tokio::sync::mpsc::{Receiver, channel},
};
use lum_log::error;
use thiserror::Error;

use crate::{
    Subscriber,
    id::get_unique_id,
    subscriber::{Callback, DispatchError},
};

pub struct EventInner<T: Clone + Send> {
    pub id: u64,
    pub name: String,
    pub subscribers: DashMap<u64, Subscriber<T>>,
}

impl<T: Clone + Send> EventInner<T> {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    pub fn subscribe_channel(
        &self,
        name: impl Into<String>,
        buffer: usize,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> (u64, Receiver<T>) {
        let (sender, receiver) = channel(buffer);

        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Channel(sender),
        );

        let id = subscriber.id();
        self.subscribers.insert(id, subscriber);

        (id, receiver)
    }

    pub fn subscribe_async_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> u64 {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::AsyncClosure(Box::new(closure)),
        );

        let id = subscriber.id();
        self.subscribers.insert(id, subscriber);

        id
    }

    pub fn subscribe_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> Result<(), BoxedError> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> u64 {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Closure(Box::new(closure)),
        );

        let id = subscriber.id();
        self.subscribers.insert(id, subscriber);

        id
    }

    pub fn unsubscribe(&self, id: u64) -> bool {
        let value = self.subscribers.remove(&id);
        value.is_some()
    }

    //TODO: Docs about cancelation safety. data can be dropped without reaching a channel.
    pub async fn dispatch(&self, data: T) -> Result<(), Vec<DispatchError<T>>> {
        let mut errors = Vec::new();
        let mut subscribers_to_remove = Vec::new();

        for ref_multi in self.subscribers.iter() {
            let id = *ref_multi.key();
            let subscriber = ref_multi.value();

            let data = data.clone();
            let result = subscriber.dispatch(data).await;
            if let Err(err) = result {
                //TODO: Remove log_on_error/remove_on_error -> provide closure for error handling?
                if subscriber.log_on_error() {
                    error!(
                        "Event \"{}\" failed to dispatch data to subscriber \"{}\": {}.",
                        self.name,
                        subscriber.name(),
                        err
                    );
                }

                if subscriber.remove_on_error() {
                    if subscriber.log_on_error() {
                        error!(
                            "Event \"{}\" will remove subscriber \"{}\" due to the error.",
                            self.name,
                            subscriber.name()
                        );
                    }

                    subscribers_to_remove.push(id);
                }

                errors.push(err);
            }
        }

        for id in subscribers_to_remove.into_iter() {
            self.subscribers.remove(&id);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(())
    }
}

impl<T: Clone + Send> PartialEq for EventInner<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T: Clone + Send> Eq for EventInner<T> {}

impl<T: Clone + Send> Hash for EventInner<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T: Clone + Send> Debug for EventInner<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscribers.len();

        f.debug_struct(type_name::<Self>())
            .field("id", &self.id)
            .field("name", &self.name)
            .field("subscribers", &sub_count)
            .finish()
    }
}

impl<T: Clone + Send> Display for EventInner<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscribers.len();
        let sub_word = if sub_count == 1 {
            "subscriber"
        } else {
            "subscribers"
        };

        write!(f, "Event {} ({} {})", self.name, sub_count, sub_word)
    }
}

#[derive(Debug, Error)]
pub enum EventHandleError {
    #[error("The event has been dropped.")]
    EventDropped,
}

#[derive(Clone)]
pub struct EventHandle<T: Clone + Send> {
    inner: Weak<EventInner<T>>,
}

impl<T: Clone + Send> EventHandle<T> {
    pub fn id(&self) -> Result<u64, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let id = inner.id();

        Ok(id)
    }

    pub fn name(&self) -> Result<String, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let name = inner.name().to_string();

        Ok(name)
    }

    pub fn subscriber_count(&self) -> Result<usize, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let count = inner.subscriber_count();

        Ok(count)
    }

    pub fn subscribe_channel(
        &self,
        name: impl Into<String>,
        buffer: usize,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Result<(u64, Receiver<T>), EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let result = inner.subscribe_channel(name, buffer, log_on_error, remove_on_error);

        Ok(result)
    }

    pub fn subscribe_async_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Result<u64, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let id = inner.subscribe_async_closure(name, closure, log_on_error, remove_on_error);

        Ok(id)
    }

    pub fn subscribe_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> Result<(), BoxedError> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Result<u64, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let id = inner.subscribe_closure(name, closure, log_on_error, remove_on_error);

        Ok(id)
    }

    pub fn unsubscribe(&self, id: u64) -> Result<bool, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let result = inner.unsubscribe(id);

        Ok(result)
    }

    pub async fn dispatch(
        &self,
        data: T,
    ) -> Result<Result<(), Vec<DispatchError<T>>>, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let result = inner.dispatch(data).await;

        Ok(result)
    }

    pub fn is_dropped(&self) -> bool {
        self.inner.strong_count() == 0
    }

    pub fn try_with_event<R>(
        &self,
        func: impl FnOnce(&EventInner<T>) -> R,
    ) -> Result<R, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let result = func(&inner);

        Ok(result)
    }

    pub async fn try_with_event_async<R>(
        &self,
        func: impl AsyncFnOnce(&EventInner<T>) -> R,
    ) -> Result<R, EventHandleError> {
        let inner = self.inner.upgrade().ok_or(EventHandleError::EventDropped)?;
        let result = func(&inner).await;

        Ok(result)
    }
}

impl<T: Clone + Send> From<Event<T>> for EventHandle<T> {
    fn from(event: Event<T>) -> Self {
        event.handle()
    }
}

impl<T: Clone + Send> From<&Event<T>> for EventHandle<T> {
    fn from(event: &Event<T>) -> Self {
        event.handle()
    }
}

impl<T: Clone + Send> From<&mut Event<T>> for EventHandle<T> {
    fn from(event: &mut Event<T>) -> Self {
        event.handle()
    }
}

impl<T: Clone + Send> From<&Self> for EventHandle<T> {
    fn from(event_handle: &Self) -> Self {
        event_handle.clone()
    }
}

impl<T: Clone + Send> AsRef<EventHandle<T>> for EventHandle<T> {
    fn as_ref(&self) -> &EventHandle<T> {
        self
    }
}

impl<T: Clone + Send> PartialEq for EventHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl<T: Clone + Send> PartialEq<EventInner<T>> for EventHandle<T> {
    fn eq(&self, other: &EventInner<T>) -> bool {
        match self.inner.upgrade() {
            Some(inner) => *inner == *other,
            None => false,
        }
    }
}

impl<T: Clone + Send> Eq for EventHandle<T> {}

impl<T: Clone + Send> Debug for EventHandle<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.inner.upgrade() {
            Some(inner) => <EventInner<T> as Debug>::fmt(&inner, f),
            None => f
                .debug_struct(type_name::<Self>())
                .field("inner", &"dropped")
                .finish(),
        }
    }
}

impl<T: Clone + Send> Display for EventHandle<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.inner.upgrade() {
            Some(inner) => <EventInner<T> as Display>::fmt(&inner, f),
            None => write!(f, "EventHandle (dropped)"),
        }
    }
}

pub struct Event<T: Clone + Send> {
    inner: Arc<EventInner<T>>,
}

impl<T: Clone + Send> Event<T> {
    pub fn new(name: impl Into<String>) -> Self {
        let id = get_unique_id();
        let name = name.into();

        let inner = EventInner {
            id,
            name,
            subscribers: DashMap::new(),
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn handle(&self) -> EventHandle<T> {
        let weak = Arc::downgrade(&self.inner);
        EventHandle { inner: weak }
    }
}

impl<T: Clone + Send> Deref for Event<T> {
    type Target = EventInner<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Clone + Send> PartialEq for Event<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: Clone + Send> Eq for Event<T> {}

impl<T: Clone + Send> PartialEq<EventHandle<T>> for Event<T> {
    fn eq(&self, other: &EventHandle<T>) -> bool {
        match other.inner.upgrade() {
            Some(other_inner) => self.inner == other_inner,
            None => false,
        }
    }
}

impl<T: Clone + Send> Hash for Event<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl<T: Clone + Send> Debug for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <EventInner<T> as Debug>::fmt(&self.inner, f)
    }
}

impl<T: Clone + Send> Display for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <EventInner<T> as Display>::fmt(&self.inner, f)
    }
}
