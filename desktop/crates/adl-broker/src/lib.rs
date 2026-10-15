//! Agentic Desktop Layer broker: authorization, leases, input engine, audit and adapters.

pub mod adapter;
pub mod audit;
pub mod broker;
pub mod mock;
pub mod recorder;
pub mod store;
pub mod training_bundle;
pub mod util;

pub use adapter::{Adapter, InputEvent, SessionEvent};
pub use broker::{Broker, Config, EnableChallenge, Peer, Principal};
pub use recorder::{StartRequest, TrainingConfig};
