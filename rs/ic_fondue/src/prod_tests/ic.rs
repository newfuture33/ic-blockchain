use super::bootstrap::{init_ic, setup_and_start_vms};
use super::driver_setup::DriverContext;
use super::resource::{allocate_resources, get_resource_request};
use super::test_setup::create_ic_handle;
use crate::ic_instance::node_software_version::NodeSoftwareVersion;
use crate::ic_manager::IcHandle;
use anyhow::Result;
use ic_protobuf::registry::subnet::v1::GossipConfig;
use ic_protobuf::registry::subnet::v1::SubnetFeatures;
use ic_registry_subnet_type::SubnetType;
use ic_types::p2p::build_default_gossip_config;
use ic_types::{Height, PrincipalId};
use phantom_newtype::AmountOf;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

/// Builder object to declare a topology of an InternetComputer. Used as input
/// to the IC Manager.
#[derive(Clone, Debug, Default)]
pub struct InternetComputer {
    pub initial_version: Option<NodeSoftwareVersion>,
    pub vm_allocation: Option<VmAllocation>,
    pub subnets: Vec<Subnet>,
    pub node_operator: Option<PrincipalId>,
    pub node_provider: Option<PrincipalId>,
    pub unassigned_nodes: Vec<Node>,
    pub ssh_readonly_access_to_unassigned_nodes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum VmAllocation {
    #[serde(rename = "distributeWithinSingleHost")]
    DistributeWithinSingleHost,
    #[serde(rename = "distributeAcrossDcs")]
    DistributeAcrossDcs,
}

impl InternetComputer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_subnet(mut self, subnet: Subnet) -> Self {
        self.subnets.push(subnet);
        self
    }

    /// Adds a one-node subnet that's optimized to be "fast".
    ///
    /// The subnet is able to execute calls faster because the block time
    /// on the node is reduced.
    pub fn add_fast_single_node_subnet(mut self, subnet_type: SubnetType) -> Self {
        self.subnets.push(Subnet::fast_single_node(subnet_type));
        self
    }

    pub fn with_initial_replica(mut self, initial_replica: NodeSoftwareVersion) -> Self {
        self.initial_version = Some(initial_replica);
        self
    }

    pub fn with_node_operator(mut self, principal_id: PrincipalId) -> Self {
        self.node_operator = Some(principal_id);
        self
    }

    pub fn with_node_provider(mut self, principal_id: PrincipalId) -> Self {
        self.node_provider = Some(principal_id);
        self
    }

    pub fn with_unassigned_nodes(mut self, no_of_nodes: i32) -> Self {
        for _ in 0..no_of_nodes {
            let node = Node::new();
            self.unassigned_nodes.push(node);
        }
        self
    }

    pub fn setup_and_start(
        &self,
        ctx: &DriverContext,
        temp_dir: &tempfile::TempDir,
        group_name: &str,
    ) -> Result<IcHandle> {
        let res_request = get_resource_request(ctx, self, group_name);
        let res_group = allocate_resources(ctx, &res_request)?;
        let (init_ic, node_vms) = init_ic(ctx, temp_dir.path(), self, &res_group);
        setup_and_start_vms(ctx, &init_ic, &node_vms)?;
        Ok(create_ic_handle(ctx, &init_ic, &node_vms))
    }
}

/// A builder for the initial configuration of a subnetwork.
#[derive(Clone, Debug, PartialEq)]
pub struct Subnet {
    pub nodes: Vec<Node>,
    pub max_ingress_bytes_per_message: Option<u64>,
    pub ingress_bytes_per_block_soft_cap: Option<u64>,
    pub max_ingress_messages_per_block: Option<u64>,
    pub max_block_payload_size: Option<u64>,
    pub unit_delay: Option<Duration>,
    pub initial_notary_delay: Option<Duration>,
    pub dkg_interval_length: Option<Height>,
    pub dkg_dealings_per_block: Option<usize>,
    // NOTE: Some values in this config, like the http port,
    // are overwritten in `update_and_write_node_config`.
    pub gossip_config: GossipConfig,
    pub subnet_type: SubnetType,
    pub max_instructions_per_message: Option<u64>,
    pub max_instructions_per_round: Option<u64>,
    pub max_instructions_per_install_code: Option<u64>,
    pub features: Option<SubnetFeatures>,
    pub max_number_of_canisters: Option<u64>,
    pub ssh_readonly_access: Vec<String>,
    pub ssh_backup_access: Vec<String>,
}

impl Subnet {
    pub fn new(subnet_type: SubnetType) -> Self {
        Self {
            nodes: vec![],
            max_ingress_bytes_per_message: None,
            ingress_bytes_per_block_soft_cap: None,
            max_ingress_messages_per_block: None,
            max_block_payload_size: None,
            unit_delay: None,
            initial_notary_delay: None,
            dkg_interval_length: None,
            dkg_dealings_per_block: None,
            gossip_config: build_default_gossip_config(),
            max_instructions_per_message: None,
            max_instructions_per_round: None,
            max_instructions_per_install_code: None,
            features: None,
            max_number_of_canisters: None,
            subnet_type,
            ssh_readonly_access: vec![],
            ssh_backup_access: vec![],
        }
    }

    /// An empty subnet that's optimized to be "fast".
    ///
    /// The subnet is able to execute calls faster because the block time
    /// on its nodes is reduced.
    ///
    /// See also `fast_single_node`.
    pub fn fast(subnet_type: SubnetType, no_of_nodes: usize) -> Self {
        assert!(
            0 < no_of_nodes,
            "cannot create subner with {} nodes",
            no_of_nodes
        );
        Self::new(subnet_type)
            // Shorter block time.
            .with_unit_delay(Duration::from_millis(200))
            .with_initial_notary_delay(Duration::from_millis(500))
            .add_nodes(no_of_nodes)
    }

    /// A one-node subnet that's optimized to be "fast".
    pub fn fast_single_node(subnet_type: SubnetType) -> Self {
        Self::fast(subnet_type, 1)
    }

    /// A (many-node) that's optimized to be "slow" so that its nodes
    /// can be run on a single machine without issues.
    ///
    /// Running many replicas on one machine means that those replicas will
    /// compete for the resources on that machine. The consensus delays
    /// essentially determine how fast the blocks are proposed and how fast the
    /// proposed blocks are notarized. If enough replicas get to run their
    /// notarizer before it is time for the next blockmaker to propose, that
    /// single block gets notarized and eventually finalized. If the system is
    /// so loaded that it is already time for the second blockmaker to propose a
    /// block before the first block gets notarized, this adds to the load of
    /// the system and points to the fact that consensus is struggling to make
    /// easy progress with the given parameters. We call this situation
    /// starvation of consensus. When the delays are increased, the amount of
    /// work that consensus attempts to make in any given time interval is
    /// decreased. This gives the first block more time to be notarized by
    /// enough replicas and possibly avoids the additional load of
    /// making/checking/notarizing multiple blocks per height. A slower
    /// consensus is therefore preferable while running multiple replicas on a
    /// single machine.
    pub fn slow(subnet_type: SubnetType) -> Self {
        Self::new(subnet_type)
            // Shorter block time.
            .with_unit_delay(Duration::from_millis(1000))
            .with_initial_notary_delay(Duration::from_millis(5000))
    }

    pub fn add_nodes(mut self, no_of_nodes: usize) -> Self {
        (0..no_of_nodes).for_each(|_| self.nodes.push(Default::default()));
        self
    }

    pub fn add_node(mut self, node: Node) -> Self {
        self.nodes.push(node);
        self
    }

    pub fn with_max_ingress_message_size(mut self, limit: u64) -> Self {
        self.max_ingress_bytes_per_message = Some(limit);
        self
    }

    pub fn with_max_block_payload_size(mut self, limit: u64) -> Self {
        self.max_block_payload_size = Some(limit);
        self
    }

    pub fn with_ingress_bytes_per_block_soft_cap(mut self, limit: u64) -> Self {
        self.ingress_bytes_per_block_soft_cap = Some(limit);
        self
    }

    pub fn with_unit_delay(mut self, unit_delay: Duration) -> Self {
        self.unit_delay = Some(unit_delay);
        self
    }

    pub fn with_initial_notary_delay(mut self, initial_notary_delay: Duration) -> Self {
        self.initial_notary_delay = Some(initial_notary_delay);
        self
    }

    pub fn with_dkg_interval_length(mut self, dkg_interval_length: Height) -> Self {
        self.dkg_interval_length = Some(dkg_interval_length);
        self
    }

    pub fn with_features(mut self, features: SubnetFeatures) -> Self {
        self.features = Some(features);
        self
    }

    pub fn with_max_number_of_canisters(mut self, max_number_of_canisters: u64) -> Self {
        self.max_number_of_canisters = Some(max_number_of_canisters);
        self
    }

    /// provides a small summary of this subnet topology and config to be used
    /// as a part of a test environment identifier.
    pub fn summary(&self) -> String {
        let ns = self.nodes.len();
        let mut s = DefaultHasher::new();
        format!("{:?}", self).hash(&mut s);
        let config_hash = format!("{:x}", s.finish());
        format!("S{:02}{}", ns, &config_hash[0..3])
    }
}

impl Default for Subnet {
    fn default() -> Self {
        Self {
            nodes: vec![],
            max_ingress_bytes_per_message: None,
            ingress_bytes_per_block_soft_cap: None,
            max_ingress_messages_per_block: None,
            max_block_payload_size: None,
            unit_delay: Some(Duration::from_millis(200)),
            initial_notary_delay: None,
            dkg_interval_length: None,
            dkg_dealings_per_block: None,
            gossip_config: build_default_gossip_config(),
            subnet_type: SubnetType::System,
            max_instructions_per_message: None,
            max_instructions_per_round: None,
            max_instructions_per_install_code: None,
            features: None,
            max_number_of_canisters: None,
            ssh_readonly_access: vec![],
            ssh_backup_access: vec![],
        }
    }
}

pub type NrOfVCPUs = AmountOf<VCPUs, u64>;
pub type AmountOfMemoryKiB = AmountOf<MemoryKiB, u64>;

pub enum VCPUs {}
pub enum MemoryKiB {}

/// A builder for the initial configuration of a node.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Node {
    pub vcpus: Option<NrOfVCPUs>,
    pub memory_kibibytes: Option<AmountOfMemoryKiB>,
}

impl Node {
    pub fn new() -> Self {
        Default::default()
    }
}
