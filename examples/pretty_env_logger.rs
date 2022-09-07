use log::{debug, error, info, trace, warn};
use std::time::SystemTime;

fn main() {
    pretty_env_logger::init_timed();

    let start = SystemTime::now();
    for i in 0..1000000 {
        match i % 5 {
            0 => error!("{}", i),
            1 => warn!("{}", i),
            2 => info!("{}", i),
            3 => debug!("{}", i),
            4 => trace!("{}", i),
            _ => panic!("can't reach here"),
        }
    }
    println!("{:?}", start.elapsed().unwrap());
}