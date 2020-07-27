use crate::segment_manager::segment_holder::{SegmentHolder, LockedSegment};
use std::sync::{RwLock, Arc};
use crate::segment_manager::segment_managers::{SegmentSearcher};
use crate::collection::{OperationResult, CollectionInfo};
use segment::types::{Filter, SearchParams, ScoredPoint, Distance, VectorElementType, PointIdType, SeqNumberType};
use tokio::runtime::Handle;
use std::collections::{HashSet, HashMap};
use segment::spaces::tools::peek_top_scores_iterable;
use futures::future::try_join_all;
use crate::operations::types::Record;

/// Simple implementation of segment manager
///  - owens segments
///  - rebuild segment for memory optimization purposes
///  - Holds information regarding id mapping to segments
///
struct SimpleSegmentSearcher {
    segments: Arc<RwLock<SegmentHolder>>,
    distance: Distance,
    runtime_handle: Handle,
}

impl SimpleSegmentSearcher {
    pub fn new(segments: Arc<RwLock<SegmentHolder>>, runtime_handle: Handle, distance: Distance) -> Self {
        return SimpleSegmentSearcher {
            segments,
            distance,
            runtime_handle,
        };
    }

    pub async fn search_in_segment(
        segment: LockedSegment,
        vector: &Vec<VectorElementType>,
        filter: Option<&Filter>,
        top: usize,
        params: Option<&SearchParams>,
    ) -> Result<Vec<ScoredPoint>, String> {
        segment.read()
            .or(Err("Unable to unlock segment".to_owned()))
            .and_then(|s| Ok(s.search(vector, filter, top, params)))
    }
}

impl SegmentSearcher for SimpleSegmentSearcher {
    fn info(&self) -> OperationResult<CollectionInfo> {
        unimplemented!()
    }

    fn search(
        &self,
        vector: &Vec<VectorElementType>,
        filter: Option<&Filter>,
        top: usize,
        params: Option<&SearchParams>,
    ) -> Vec<ScoredPoint> {
        let searches: Vec<_> = self.segments
            .read()
            .unwrap()
            .iter()
            .map(|(_id, segment)|
                SimpleSegmentSearcher::search_in_segment(segment.clone(), vector, filter, top, params))
            .collect();

        let all_searches = try_join_all(searches);
        let all_search_results: Vec<Vec<ScoredPoint>> = self.runtime_handle.block_on(all_searches).unwrap();
        let mut seen_idx: HashSet<PointIdType> = HashSet::new();

        peek_top_scores_iterable(
            all_search_results
                .iter()
                .flatten()
                .filter(|scored| {
                    let res = seen_idx.contains(&scored.idx);
                    seen_idx.insert(scored.idx);
                    !res
                })
                .cloned(),
            top,
            &self.distance,
        )
    }

    fn retrieve(&self, points: &Vec<PointIdType>, with_payload: bool, with_vector: bool) -> Vec<Record> {
        let mut point_version: HashMap<PointIdType, SeqNumberType> = Default::default();
        let mut point_records: HashMap<PointIdType, Record> = Default::default();
        self.segments.read().unwrap().read_points(points, |id, segment| {
            /// If this point was not found yet or this segment have later version
            if !point_version.contains_key(&id) || point_version[&id] < segment.version() {
                point_records.insert(id, Record {
                    id,
                    payload: if with_payload { Some(segment.payload(id)?) } else { None },
                    vector: if with_vector { Some(segment.vector(id)?) } else { None },
                });
                point_version.insert(id, segment.version());
            }
            Ok(true)
        });
        point_records.into_iter().map(|(_, r)| r).collect()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;
    use tokio::runtime;
    use crate::segment_manager::fixtures::build_test_holder;

    #[test]
    fn test_segments_search() {
        let segment_holder = build_test_holder();

        let threaded_rt1: Runtime = runtime::Builder::new()
            .threaded_scheduler()
            .max_threads(2)
            .build().unwrap();


        let searcher = SimpleSegmentSearcher::new(
            Arc::new(RwLock::new(segment_holder)),
            threaded_rt1.handle().clone(),
            Distance::Dot,
        );

        let query = vec![1.0, 1.0, 1.0, 1.0];

        let result = searcher.search(
            &query,
            None,
            5,
            None,
        );

        // eprintln!("result = {:?}", &result);

        assert_eq!(result.len(), 5);

        assert!(result[0].idx == 3 || result[0].idx == 11);
        assert!(result[1].idx == 3 || result[1].idx == 11);
    }

    #[test]
    fn test_retrieve() {
        let segment_holder = build_test_holder();

        let threaded_rt1: Runtime = runtime::Builder::new()
            .threaded_scheduler()
            .max_threads(2)
            .build().unwrap();

        let searcher = SimpleSegmentSearcher::new(
            Arc::new(RwLock::new(segment_holder)),
            threaded_rt1.handle().clone(),
            Distance::Dot,
        );

        let records = searcher.retrieve(&vec![1,2,3], true, true);

        assert_eq!(records.len(), 3);
    }
}