#[cfg(test)]
mod tests {

    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    };

    use lum_event::{ArcObservable, Observable};
    use lum_libs::tokio::{self};

    static TEST_EVENT_NAME: &str = "test_event";
    static TEST_CLOSURE_NAME: &str = "test_closure";
    static TEST_DATA: &str = "test_data";
    static TEST_DATA_INITIAL: &str = "Did not trigger";

    #[tokio::test]
    async fn observable_new() {
        let observable = Observable::new(TEST_DATA, TEST_EVENT_NAME);
        let data = observable.get();
        let data_as_ref: &str = observable.as_ref();

        assert_eq!(observable, TEST_DATA);
        assert_eq!(data, TEST_DATA);
        assert_eq!(data_as_ref, TEST_DATA);
    }

    #[tokio::test]
    async fn observable_subscriber_receives_data_on_change() {
        let mut observable = Observable::new(TEST_DATA_INITIAL, TEST_EVENT_NAME);
        let count = Arc::new(AtomicU8::new(0));

        let count_clone = count.clone();
        let uuid = observable
            .on_change
            .subscribe_closure(
                TEST_CLOSURE_NAME,
                move |data| {
                    assert_eq!(data, TEST_DATA);
                    count_clone.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
                false,
                false,
            )
            .await;
        assert_eq!(observable.on_change.subscriber_count().await, 1);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.on_change.unsubscribe(&uuid).await;
        assert_eq!(observable.on_change.subscriber_count().await, 0);

        observable.set(TEST_DATA_INITIAL).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn arc_observable_new() {
        let observable = ArcObservable::new(TEST_DATA, TEST_EVENT_NAME);
        let data = observable.get();

        {
            let lock = data.blocking_lock();
            assert_eq!(*lock, TEST_DATA);
        }

        assert_eq!(observable, TEST_DATA);
    }

    #[tokio::test]
    async fn arc_observable_subscriber_receives_data_on_change() {
        let observable = ArcObservable::new(TEST_DATA_INITIAL, TEST_EVENT_NAME);

        let count = Arc::new(AtomicU8::new(0));

        let count_clone = count.clone();
        let uuid = observable
            .on_change
            .subscribe_closure(
                TEST_CLOSURE_NAME,
                move |data| {
                    let lock = data.try_lock().unwrap();
                    assert_eq!(*lock, TEST_DATA);
                    count_clone.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
                false,
                false,
            )
            .await;
        assert_eq!(observable.on_change.subscriber_count().await, 1);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.on_change.unsubscribe(&uuid).await;
        assert_eq!(observable.on_change.subscriber_count().await, 0);

        observable.set(TEST_DATA_INITIAL).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
