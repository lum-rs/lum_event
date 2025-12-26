use lum_boxtypes::{BoxedError, BoxedErrorResult, PinnedBoxedFutureResult};
use lum_libs::tokio::sync::mpsc::{Sender, error::SendError};
use thiserror::Error;

use crate::id::get_unique_id;

pub enum Callback<T> {
    Channel(Sender<T>),
    Closure(Box<dyn Fn(T) -> BoxedErrorResult<()> + Send + Sync>),
    AsyncClosure(Box<dyn Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync>),
}

#[derive(Debug, Error)]
pub enum DispatchError<T> {
    #[error("Failed to send data to channel: {0}")]
    ChannelSend(#[from] SendError<T>),

    #[error("Failed to dispatch data to closure: {0}")]
    Closure(BoxedError),

    #[error("Failed to dispatch data to async closure: {0}")]
    AsyncClosure(BoxedError),
}

pub struct Subscriber<T: Clone + Send> {
    pub name: String,
    pub log_on_error: bool,
    pub remove_on_error: bool,
    pub callback: Callback<T>,
    pub id: u64,
}

impl<T: Clone + Send> Subscriber<T> {
    pub fn new(
        name: impl Into<String>,
        log_on_error: bool,
        remove_on_error: bool,
        callback: Callback<T>,
    ) -> Self {
        let id = get_unique_id();
        Self {
            name: name.into(),
            log_on_error,
            remove_on_error,
            callback,
            id,
        }
    }

    //TODO: For closure callback, consider spawning a task to avoid blocking. Or defining a ClosureNonBlocking variant.
    //TODO: Docs about cancelation safety. data can be dropped without reaching a channel.
    pub async fn dispatch(&self, data: T) -> Result<(), DispatchError<T>> {
        match &self.callback {
            Callback::Channel(sender) => {
                sender.send(data).await.map_err(DispatchError::ChannelSend)
            }
            Callback::Closure(closure) => closure(data).map_err(DispatchError::Closure),
            Callback::AsyncClosure(closure) => {
                closure(data).await.map_err(DispatchError::AsyncClosure)
            }
        }
    }
}

impl<T: Clone + Send> PartialEq for Subscriber<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T: Clone + Send> Eq for Subscriber<T> {}
