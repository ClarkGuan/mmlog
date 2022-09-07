use lazy_static::lazy_static;
use log::{debug, error, info, trace, warn, LevelFilter};
use mmlog::{Builder, Logger, MB};
use std::time::SystemTime;

lazy_static! {
    static ref LOGGER: Logger = {
        let b = Builder::new().size(5 * MB);
        if matches!(std::fs::metadata("test.log"), Ok(_)) {
            b.open("test.log").expect("Builder::open()")
        } else {
            b.build("test.log").expect("Builder::build()")
        }
    };
}

fn main() {
    log::set_logger(&*LOGGER).expect("log::set_logger()");
    log::set_max_level(LevelFilter::Trace);
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
