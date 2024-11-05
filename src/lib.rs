use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::{
    collections::HashMap,
    result::Result,
    sync::{Arc, Mutex},
};
use tokio::spawn;

type City = String;
type Temperature = u64;

#[async_trait]
pub trait Api: Send + Sync + 'static {
    async fn fetch(&self) -> Result<HashMap<City, Temperature>, String>;
    async fn subscribe(&self) -> BoxStream<Result<(City, Temperature), String>>;
}

pub struct StreamCache {
    results: Arc<Mutex<HashMap<String, u64>>>,
}

impl StreamCache {
    pub fn new(api: impl Api) -> Self {
        let instance = Self {
            results: Arc::new(Mutex::new(HashMap::new())),
        };
        instance.update_in_background(api);
        instance
    }

    pub fn get(&self, key: &str) -> Option<u64> {
        let results = self.results.lock().expect("poisoned");
        results.get(key).copied()
    }

    fn fetch_in_background(&self, api_arc: &Arc<impl Api>) {
        let results = self.results.clone();
        let api = Arc::clone(api_arc);

        spawn(async move {
            // Step 1: Initial fetch to populate the cache
            match api.fetch().await {
                Ok(initial_data) => {
                    let mut cache = results.lock().expect("poisoned");
                    for (city, temperature) in initial_data {
                        // cache.insert(city, temperature);
                        cache.entry(city).or_insert(temperature); // prioritize 'subscribe'
                    }
                }
                Err(e) => {
                    eprintln!("Failed to perform initial fetch: {}", e);
                }
            }
        });        
    }

    fn subscribe_in_background(&self, api_arc: &Arc<impl Api>) {
        let results = self.results.clone();
        let api = Arc::clone(api_arc);

        spawn(async move {
            // Step 2: Subscribe to real-time updates
            let mut updates = api.subscribe().await;

            while let Some(update) = updates.next().await {
                match update {
                    Ok((city, temperature)) => {
                        let mut cache = results.lock().expect("poisoned");
                        cache.insert(city, temperature);
                    }
                    Err(e) => {
                        eprintln!("Failed to get update from subscribe: {}", e);
                    }
                }
            }
        });
    }

    pub fn update_in_background(&self, api: impl Api) {
        let api_arc = Arc::new(api);
        self.fetch_in_background(&api_arc);
        self.subscribe_in_background(&api_arc);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::sync::Notify;
    use tokio::time;

    use futures::{future, stream::select, FutureExt, StreamExt};
    use maplit::hashmap;

    use super::*;

    #[derive(Default)]
    struct TestApi {
        signal: Arc<Notify>,
    }

    #[async_trait]
    impl Api for TestApi {
        async fn fetch(&self) -> Result<HashMap<City, Temperature>, String> {
            // fetch is slow an may get delayed until after we receive the first updates
            self.signal.notified().await;
            Ok(hashmap! {
                "Berlin".to_string() => 29,
                "Paris".to_string() => 31,
            })
        }
        async fn subscribe(&self) -> BoxStream<Result<(City, Temperature), String>> {
            let results = vec![
                Ok(("London".to_string(), 27)),
                Ok(("Paris".to_string(), 32)),
                Ok(("Riga".to_string(), 20)),
                Ok(("Riga".to_string(), 19)),
            ];
            select(
                futures::stream::iter(results),
                async {
                    self.signal.notify_one();
                    future::pending().await
                }
                .into_stream(),
            )
            .boxed()
        }
    }
    #[tokio::test]
    async fn works() {
        let cache = StreamCache::new(TestApi::default());

        // Allow cache to update
        time::sleep(Duration::from_millis(100)).await;

        assert_eq!(cache.get("Berlin"), Some(29));
        assert_eq!(cache.get("London"), Some(27));
        assert_eq!(cache.get("Paris"), Some(32));
        assert_eq!(cache.get("Riga"), Some(19));
        assert_eq!(cache.get("Tallin"), None);
    }
}
