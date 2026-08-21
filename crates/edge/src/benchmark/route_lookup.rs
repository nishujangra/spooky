use std::collections::HashMap;

use impulse_config::config::Upstream;

use crate::{
    benchmark::helpers::build_benchmark_upstream,
    routing::{index::RouteIndex, scan::scan_lookup},
};

pub struct RouteLookupBench {
    upstreams: HashMap<String, Upstream>,
    index: RouteIndex,
    hit_path: String,
    hit_host: Option<String>,
    miss_path: String,
    miss_host: Option<String>,
}

impl RouteLookupBench {
    pub fn new(scale: usize) -> Self {
        let mut upstreams = HashMap::with_capacity(scale.max(1));
        for i in 0..scale.max(1) {
            let name = format!("upstream-{i:05}");
            let path_prefix = format!("/svc/{i:05}");
            let host = (i % 2 == 1).then_some("bench.example.com".to_string());
            upstreams.insert(name, build_benchmark_upstream(host, path_prefix));
        }

        let index = RouteIndex::from_upstreams(&upstreams);
        let target = scale.max(1) - 1;
        let hit_path = format!("/svc/{target:05}/resource");
        let hit_host = (target % 2 == 1).then_some("bench.example.com".to_string());
        let miss_path = "/not-found/path".to_string();
        let miss_host = Some("missing.example.com".to_string());

        Self {
            upstreams,
            index,
            hit_path,
            hit_host,
            miss_path,
            miss_host,
        }
    }

    pub fn indexed_hit(&self) -> usize {
        self.index
            .lookup(&self.hit_path, self.hit_host.as_deref())
            .map_or(0, str::len)
    }

    pub fn linear_hit(&self) -> usize {
        scan_lookup(&self.upstreams, &self.hit_path, self.hit_host.as_deref()).map_or(0, str::len)
    }

    pub fn indexed_miss(&self) -> usize {
        self.index
            .lookup(&self.miss_path, self.miss_host.as_deref())
            .map_or(0, str::len)
    }
}
