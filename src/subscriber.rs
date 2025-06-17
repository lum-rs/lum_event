use lum_boxtypes::{BoxedError, BoxedErrorResult, PinnedBoxedFutureResult};
use lum_libs::{
    tokio::sync::mpsc::{Sender, error::SendError},
    uuid::Uuid,
};
use thiserror::Error;

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

pub struct Subscriber<T>
where
    T: Clone + Send,
{
    pub name: String,
    pub log_on_error: bool,
    pub remove_on_error: bool,
    pub callback: Callback<T>,

    pub uuid: Uuid,
}

impl<T> Subscriber<T>
where
    T: Clone + Send,
{
    pub fn new(
        name: impl Into<String>,
        log_on_error: bool,
        remove_on_error: bool,
        callback: Callback<T>,
    ) -> Self {
        Self {
            name: name.into(),
            log_on_error,
            remove_on_error,
            callback,
            uuid: Uuid::new_v4(),
        }
    }

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

impl<T> PartialEq for Subscriber<T>
where
    T: Clone + Send,
{
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<T> PartialEq<Uuid> for Subscriber<T>
where
    T: Clone + Send,
{
    fn eq(&self, other: &Uuid) -> bool {
        self.uuid == *other
    }
}

impl<T> Eq for Subscriber<T> where T: Clone + Send {}
