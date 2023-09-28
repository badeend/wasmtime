use crate::preview2::bindings::sockets::network::Network;
use crate::preview2::{Table, TableError};
use cap_std::net::Pool;
use wasmtime::component::Resource;

pub struct HostNetworkState {
    pub pool: Pool,
    pub allow_ip_name_lookup: bool,
}

pub(crate) trait TableNetworkExt {
    fn push_network(&mut self, network: HostNetworkState) -> Result<Resource<Network>, TableError>;
    fn delete_network(&mut self, fd: Resource<Network>) -> Result<HostNetworkState, TableError>;
    fn is_network(&self, fd: &Resource<Network>) -> bool;
    fn get_network(&self, fd: &Resource<Network>) -> Result<&HostNetworkState, TableError>;
}

impl TableNetworkExt for Table {
    fn push_network(&mut self, network: HostNetworkState) -> Result<Resource<Network>, TableError> {
        Ok(Resource::new_own(self.push(Box::new(network))?))
    }
    fn delete_network(&mut self, fd: Resource<Network>) -> Result<HostNetworkState, TableError> {
        self.delete(fd.rep())
    }
    fn is_network(&self, fd: &Resource<Network>) -> bool {
        self.is::<HostNetworkState>(fd.rep())
    }
    fn get_network(&self, fd: &Resource<Network>) -> Result<&HostNetworkState, TableError> {
        self.get(fd.rep())
    }
}
