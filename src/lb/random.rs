
// Random load balancing strategy implementation
// 
// TODO: Add weighted random selection support
// TODO: Implement health check integration
// TODO: Add metrics collection for random selection
// TODO: Handle server failure scenarios
// TODO: Add configuration for random seed
// 

use log::{error, info};

use rand::{seq::SliceRandom, thread_rng};

use crate::config::config::{Backend};

pub fn random(backends: &[Backend]) -> Option<&Backend> {
    let mut rng = thread_rng();

    let healthy_backends: Vec<&Backend> = backends.iter().filter(|b| b.is_healthy()).collect();

    match healthy_backends.choose(&mut rng) {
        Some(random_backend) => {
            info!("Selected backend address: {}", random_backend.address);
            Some(random_backend)
        }
        None => {
            error!("No backend avaliable");
            None
        }
    }

}