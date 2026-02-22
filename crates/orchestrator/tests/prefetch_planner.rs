#![forbid(unsafe_code)]

use config::{Config, MemoryPolicy, SortStrategy};
use half::f16;
use orchestrator::domain::{MapSegment, MemStat};
use orchestrator::prediction::Prediction;
use orchestrator::prefetch::GreedyPrefetchPlanner;
use orchestrator::prefetch::PrefetchPlanner;
use orchestrator::stores::Stores;
use std::os::linux::fs::MetadataExt;
use tempfile::tempdir;

fn segment_with_meta(
    path: impl Into<std::path::PathBuf>,
    offset: u64,
    length: u64,
    device: u64,
    inode: u64,
) -> MapSegment {
    let mut seg = MapSegment::new(path, offset, length, 0);
    seg.device = device;
    seg.inode = inode;
    seg
}

#[test]
fn planner_selects_maps_within_budget() {
    let mut config = Config::default();
    config.model.memory = MemoryPolicy {
        memtotal: 0,
        memavailable: 100,
    };
    config.system.sortstrategy = SortStrategy::None;

    let planner = GreedyPrefetchPlanner::new(&config);
    let mut stores = Stores::default();

    let map_a = stores.ensure_map(MapSegment::new("/a", 0, 2048, 0));
    let map_b = stores.ensure_map(MapSegment::new("/b", 0, 2048, 0));
    let map_c = stores.ensure_map(MapSegment::new("/c", 0, 1024, 0));

    let mut prediction = Prediction::default();
    prediction.map_scores.insert(map_a, f16::from_f32(0.9));
    prediction.map_scores.insert(map_b, f16::from_f32(0.8));
    prediction.map_scores.insert(map_c, f16::from_f32(0.7));

    let mem = MemStat {
        total: 0,
        available: 3,
        free: 3,
        cached: 0,
        pagein: 0,
        pageout: 0,
    };

    let plan = planner.plan(&prediction, &stores, &mem);

    assert_eq!(plan.maps.len(), 2);
    assert!(plan.maps.contains(&map_a));
    assert!(plan.maps.contains(&map_c));
    assert!(!plan.maps.contains(&map_b));
    assert_eq!(plan.total_bytes, 2048 + 1024);
    assert_eq!(plan.budget_bytes, 3 * 1024);
}

#[test]
fn planner_sorts_by_block_with_score_tiebreak() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("data.bin");
    std::fs::write(&path, vec![0u8; 16 * 1024]).unwrap();

    let meta = std::fs::metadata(&path).unwrap();
    let device = meta.st_dev();
    let inode = meta.st_ino();

    let mut config = Config::default();
    config.model.memory = MemoryPolicy {
        memtotal: 0,
        memavailable: 100,
    };
    config.system.sortstrategy = SortStrategy::Block;

    let planner = GreedyPrefetchPlanner::new(&config);
    let mut stores = Stores::default();

    let map_a = stores.ensure_map(segment_with_meta(&path, 8192, 1024, device, inode));
    let map_b = stores.ensure_map(segment_with_meta(&path, 0, 1024, device, inode));
    let map_c = stores.ensure_map(segment_with_meta(&path, 4096, 1024, device, inode));

    let mut prediction = Prediction::default();
    prediction.map_scores.insert(map_a, f16::ONE);
    prediction.map_scores.insert(map_b, f16::ONE);
    prediction.map_scores.insert(map_c, f16::ONE);

    let mem = MemStat {
        total: 0,
        available: 64,
        free: 64,
        cached: 0,
        pagein: 0,
        pageout: 0,
    };

    let plan = planner.plan(&prediction, &stores, &mem);

    assert_eq!(plan.maps, vec![map_b, map_c, map_a]);
}

#[test]
fn planner_sorts_by_inode_with_score_tiebreak() {
    let dir = tempdir().unwrap();
    let path_a = dir.path().join("a.bin");
    let path_b = dir.path().join("b.bin");
    std::fs::write(&path_a, vec![0u8; 4096]).unwrap();
    std::fs::write(&path_b, vec![1u8; 4096]).unwrap();

    let meta_a = std::fs::metadata(&path_a).unwrap();
    let meta_b = std::fs::metadata(&path_b).unwrap();
    let device_a = meta_a.st_dev();
    let inode_a = meta_a.st_ino();
    let device_b = meta_b.st_dev();
    let inode_b = meta_b.st_ino();

    let mut config = Config::default();
    config.model.memory = MemoryPolicy {
        memtotal: 0,
        memavailable: 100,
    };
    config.system.sortstrategy = SortStrategy::Inode;

    let planner = GreedyPrefetchPlanner::new(&config);
    let mut stores = Stores::default();

    let map_a = stores.ensure_map(segment_with_meta(&path_a, 0, 1024, device_a, inode_a));
    let map_b = stores.ensure_map(segment_with_meta(&path_b, 0, 1024, device_b, inode_b));

    let mut prediction = Prediction::default();
    prediction.map_scores.insert(map_a, f16::ONE);
    prediction.map_scores.insert(map_b, f16::ONE);

    let mem = MemStat {
        total: 0,
        available: 64,
        free: 64,
        cached: 0,
        pagein: 0,
        pageout: 0,
    };

    let plan = planner.plan(&prediction, &stores, &mem);

    let expected = if inode_a <= inode_b {
        vec![map_a, map_b]
    } else {
        vec![map_b, map_a]
    };
    assert_eq!(plan.maps, expected);
}
