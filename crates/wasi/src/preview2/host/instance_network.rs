use crate::preview2::bindings::sockets::instance_network::{self, Network};
use crate::preview2::network::{HostNetworkState, TableNetworkExt};
use crate::preview2::WasiView;
use wasmtime::component::Resource;

impl<T: WasiView> instance_network::Host for T {
    fn instance_network(&mut self) -> Result<Resource<Network>, anyhow::Error> {
        let network = HostNetworkState {
            pool: self.ctx().pool.clone(),
            allow_ip_name_lookup: self.ctx().allow_ip_name_lookup,
        };
        let network = self.table_mut().push_network(network)?;
        Ok(network)
    }
}
