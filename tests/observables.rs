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

    //TODO: This is a unit test. Move to observable.rs
    #[test]
    fn observable_new() {
        let observable = Observable::new(TEST_DATA, TEST_EVENT_NAME);
        let data = observable.get();
        let data_as_ref: &str = observable.as_ref();

        assert_eq!(observable, TEST_DATA);
        assert_eq!(data, TEST_DATA);
        assert_eq!(data_as_ref, TEST_DATA);
    }

    //TODO: This is an integration test that stretches across multiple components. Move to event_system.rs
    //TODO: Use sender/receiver instead?
    #[tokio::test]
    async fn observable_subscriber_receives_data_on_change() {
        let mut observable = Observable::new(TEST_DATA_INITIAL, TEST_EVENT_NAME);
        let count = Arc::new(AtomicU8::new(0));

        let count_clone = count.clone();
        let uuid = observable.on_change.subscribe_closure(
            TEST_CLOSURE_NAME,
            move |data| {
                assert_eq!(data, TEST_DATA);
                count_clone.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            false,
            false,
        );
        assert_eq!(observable.on_change.subscriber_count(), 1);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.on_change.unsubscribe(uuid);
        assert_eq!(observable.on_change.subscriber_count(), 0);

        observable.set(TEST_DATA_INITIAL).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    //TODO: This should check the observable and the value for equality, not the inside value
    //TODO: This is a unit test. Move to arc_observable.rs
    #[test]
    fn arc_observable_new() {
        let observable = ArcObservable::new(TEST_DATA, TEST_EVENT_NAME);
        let data = observable.get();

        assert_eq!(*data, TEST_DATA);
        assert_eq!(observable, TEST_DATA);
    }

    //TODO: This is an integration test that stretches across multiple components. Move to event_system.rs
    //TODO: Use sender/receiver instead?
    #[tokio::test]
    async fn arc_observable_subscriber_receives_data_on_change() {
        let observable = ArcObservable::new(TEST_DATA_INITIAL, TEST_EVENT_NAME);

        let count = Arc::new(AtomicU8::new(0));

        let count_clone = count.clone();
        let uuid = observable.on_change.subscribe_closure(
            TEST_CLOSURE_NAME,
            move |data| {
                assert_eq!(*data, TEST_DATA);
                count_clone.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            false,
            false,
        );
        assert_eq!(observable.on_change.subscriber_count(), 1);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.set(TEST_DATA).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);

        observable.on_change.unsubscribe(uuid);
        assert_eq!(observable.on_change.subscriber_count(), 0);

        observable.set(TEST_DATA_INITIAL).await;
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn observable_drop_with_no_references() {
        let observable = Observable::new(TEST_DATA, TEST_EVENT_NAME);
        assert_eq!(Arc::strong_count(&observable.on_change), 1);
        drop(observable); // Should drop cleanly without warnings
    }

    #[test]
    fn observable_drop_with_extra_references() {
        let observable = Observable::new(TEST_DATA, TEST_EVENT_NAME);
        let arc_ref = observable.on_change.clone();

        assert_eq!(Arc::strong_count(&observable.on_change), 2);

        drop(observable); // Should trigger warning (Arc count > 1)

        // Arc is still alive
        assert_eq!(Arc::strong_count(&arc_ref), 1);
        drop(arc_ref);
    }

    #[test]
    fn arc_observable_drop_with_no_references() {
        let observable = ArcObservable::new(TEST_DATA, TEST_EVENT_NAME);
        assert_eq!(Arc::strong_count(&observable.on_change), 1);
        drop(observable); // Should drop cleanly without warnings
    }

    #[test]
    fn arc_observable_drop_with_extra_references() {
        let observable = ArcObservable::new(TEST_DATA, TEST_EVENT_NAME);
        let arc_ref = observable.on_change.clone();

        assert_eq!(Arc::strong_count(&observable.on_change), 2);

        drop(observable); // Should trigger warning (Arc count > 1)

        // Arc is still alive
        assert_eq!(Arc::strong_count(&arc_ref), 1);
        drop(arc_ref);
    }
}
