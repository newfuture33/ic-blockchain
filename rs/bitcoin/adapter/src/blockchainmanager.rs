use crate::blockchainstate::AddHeaderError;
use crate::common::MINIMUM_VERSION_NUMBER;
use crate::ProcessEventError;
use crate::{
    blockchainstate::BlockchainState, common::BlockHeight, config::Config, stream::StreamEvent,
    stream::StreamEventKind, Channel, Command, HandleClientRequest,
};
use bitcoin::network::message::MAX_INV_SIZE;
use bitcoin::{
    network::message::NetworkMessage, network::message_blockdata::GetHeadersMessage,
    network::message_blockdata::Inventory, Block, BlockHash, BlockHeader,
};
use rand::prelude::*;
use slog::Logger;
use std::net::SocketAddr;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
    time::SystemTime,
};
use thiserror::Error;

// TODO: ER-2133: Unify usage of the `getdata` term when referring to the Bitcoin message.
/// This constant is the maximum number of seconds to wait until we get response to the getdata request sent by us.
const GETDATA_REQUEST_TIMEOUT_SECS: u64 = 30;

/// This constant represents the maximum size of `headers` messages.
/// https://developer.bitcoin.org/reference/p2p_networking.html#headers
const MAX_HEADERS_SIZE: usize = 2_000;

/// This constant stores the maximum number of headers allowed in an unsolicited `headers` message
/// (`headers message for which a `getheaders` request was not sent before.)
const MAX_UNSOLICITED_HEADERS: usize = 20;

///Max number of inventory in the "getdata" request that can be sent
/// to a peer at a time.
const INV_PER_GET_DATA_REQUEST: u32 = 8;

/// This value represents the number of
const FUTURE_SUCCESSORS_DEPTH: u32 = 5;

/// Block locators. Consists of starting hashes and a stop hash.
type Locators = (Vec<BlockHash>, BlockHash);

/// The enum stores what to do if a timeout for a peer is received.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum OnTimeout {
    /// Disconnect the peer on timeout.
    Disconnect,
    /// Do nothing on timeout.
    Ignore,
}

/// The possible errors the `BlockchainManager::received_headers_message(...)` may produce.
#[derive(Debug, Error)]
enum ReceivedHeadersMessageError {
    /// This variant represents when a message from a no longer known peer.
    #[error("Unknown peer")]
    UnknownPeer,
    #[error("Received too many headers (> 2000)")]
    ReceivedTooManyHeaders,
    #[error("Received too many unsolicited headers")]
    ReceivedTooManyUnsolicitedHeaders,
    #[error("Received an invalid header")]
    ReceivedInvalidHeader,
}

/// The possible errors the `BlockchainManager::received_inv_message(...)` may produce.
#[derive(Debug, Error)]
enum ReceivedInvMessageError {
    /// This variant represents when a message from a no longer known peer.
    #[error("Unknown peer")]
    UnknownPeer,
    /// The number of inventory in the message exceeds the maximum limit
    #[error("Received too many inventory items from a peer")]
    TooMuchInventory,
}

/// The possible errors the `BlockchainManager::received_block_message(...)` may produce.
#[derive(Debug, Error)]
pub enum ReceivedBlockMessageError {
    /// This variant represents when a message from a no longer known peer.
    #[error("Unknown peer")]
    UnknownPeer,
    /// This variant represents that a block was not able to be added to the `block_cache` in the
    /// BlockchainState.
    #[error("Failed to add block")]
    BlockNotAdded,
}

/// This struct stores the information regarding a peer w.r.t synchronizing blockchain.
/// This information is useful to keep track of the commands that have been sent to the peer,
/// and how much blockchain state has already been synced with the peer.
#[derive(Debug)]
pub struct PeerInfo {
    /// This field stores the socket address of the Bitcoin node (peer)
    socket: SocketAddr,
    /// This field stores the height of the last headers/data message received from the peer.
    height: BlockHeight,
    /// This field stores the block hash of the tip header received from the peer.
    tip: BlockHash,

    //last_active: Option<LocalTime>,
    /// Locators sent in the last `GetHeaders` or `GetData` request
    last_asked: Option<Locators>,
    /// Time at which the request was sent.
    sent_at: Option<SystemTime>,
    /// What to do if this request times out.
    on_timeout: OnTimeout,
    /// Number of outstanding & unexpired 'GetData' requests sent to the peer
    /// but the corresponding "Block" response not received yet.  
    num_of_outstanding_get_data_requests: u32,
}

/// This struct stores the information related to a "GetData" request sent by the BlockChainManager
#[derive(Debug)]
pub struct GetDataRequestInfo {
    /// This field stores the socket address of the Bitcoin node to which the request was sent.
    socket: SocketAddr,
    /// This field contains the inventory for which the GetData request was issued.
    inventory: Inventory,
    /// This field contains the time at which the GetData request was sent.  
    sent_at: SystemTime,
    /// This field contains the action to take if the request is expired.
    on_timeout: OnTimeout,
}

/// The BlockChainManager struct handles interactions that involve the headers.
#[derive(Debug)]
pub struct BlockchainManager {
    /// This field contains the BlockchainState, which stores and manages
    /// all the information related to the headers and blocks.
    blockchain: BlockchainState,

    /// This field stores the map of which bitcoin nodes sent which "inv" messages.
    peer_info: HashMap<SocketAddr, PeerInfo>,

    /// Random number generator used for sampling a random peer to send "GetData" request.
    rng: StdRng,

    /// This HashMap stores the information related to each get_data request
    /// sent by the BlockChainManager. An entry is removed from this hashmap if
    /// (1) The corresponding "Block" response is received or
    /// (2) If the request is expired or
    /// (3) If the peer is disconnected.
    get_data_request_info: HashMap<Inventory, GetDataRequestInfo>,

    /// This HashSet stores the list of inventory that is yet to be synced by the BlockChainManager.
    inventory_to_be_synced: HashSet<Inventory>,

    /// This vector stores the list of messages that are to be sent to the Bitcoin network.
    outgoing_command_queue: Vec<Command>,
    /// This field contains a logger for the blockchain manager's use.
    logger: Logger,
}

impl BlockchainManager {
    /// This function instantiates a BlockChainManager struct. A node is provided
    /// in order to get its client so the manager can send messages to the
    /// BTC network.
    pub fn new(config: &Config, logger: Logger) -> Self {
        let blockchain = BlockchainState::new(config);
        let peer_info = HashMap::new();
        let get_data_request_info = HashMap::new();
        let rng = StdRng::from_entropy();
        let inventory_to_be_synced = HashSet::new();
        let outgoing_command_queue = Vec::new();
        BlockchainManager {
            blockchain,
            peer_info,
            rng,
            get_data_request_info,
            inventory_to_be_synced,
            outgoing_command_queue,
            logger,
        }
    }

    /// This method sends `getheaders` command to the adapter.
    /// The adapter then sends the `getheaders` request to the Bitcoin node.
    fn send_getheaders(&mut self, addr: &SocketAddr, locators: Locators, on_timeout: OnTimeout) {
        // TODO: ER-1394: Timeouts must for getheaders calls must be handled.
        //If the peer address is not stored in peer_info, then return;
        if let Some(peer_info) = self.peer_info.get_mut(addr) {
            slog::debug!(
                self.logger,
                "Sending GetHeaders to {} : Locator hashes {:?}, Stop hash {}",
                addr,
                locators.0,
                locators.1
            );
            let command = Command {
                address: Some(*addr),
                message: NetworkMessage::GetHeaders(GetHeadersMessage {
                    locator_hashes: locators.0.clone(),
                    stop_hash: locators.1,
                    version: MINIMUM_VERSION_NUMBER,
                }),
            };
            //If sending the command is successful, then update the peer_info with the new details.
            self.outgoing_command_queue.push(command);
            // Caveat: Updating peer_info even if the command hasn't been set yet.
            peer_info.last_asked = Some(locators);
            peer_info.sent_at = Some(SystemTime::now());
            peer_info.on_timeout = on_timeout;
        }
    }

    /// This function processes "inv" messages received from Bitcoin nodes.
    /// Given a block_hash, this method sends the corresponding "GetHeaders" message to the Bitcoin node.
    fn received_inv_message(
        &mut self,
        addr: &SocketAddr,
        inventory: &[Inventory],
    ) -> Result<(), ReceivedInvMessageError> {
        // If the inv message has more inventory than MAX_INV_SIZE (50000), reject it.
        if inventory.len() > MAX_INV_SIZE {
            return Err(ReceivedInvMessageError::TooMuchInventory);
        }

        // If the inv message is received from a peer that is not connected, then reject it.
        slog::info!(
            self.logger,
            "Received inv message from {} : Inventory {:?}",
            addr,
            inventory
        );

        let peer = self
            .peer_info
            .get_mut(addr)
            .ok_or(ReceivedInvMessageError::UnknownPeer)?;

        //This field stores the block hash in the inventory that is not yet stored in the blockchain,
        // and has the highest height amongst all the hashes in the inventory.
        let mut last_block = None;

        for inv in inventory {
            if let Inventory::Block(hash) = inv {
                peer.tip = *hash;
                if !self.blockchain.is_block_hash_known(hash) {
                    last_block = Some(hash);
                }
            }
        }

        if let Some(stop_hash) = last_block {
            let locators = (self.blockchain.locator_hashes(), *stop_hash);

            // Send `GetHeaders` request to fetch the headers corresponding to inv message.
            self.send_getheaders(addr, locators, OnTimeout::Ignore);
        }
        Ok(())
    }

    fn received_headers_message(
        &mut self,
        addr: &SocketAddr,
        headers: &[BlockHeader],
    ) -> Result<(), ReceivedHeadersMessageError> {
        let peer = self
            .peer_info
            .get_mut(addr)
            .ok_or(ReceivedHeadersMessageError::UnknownPeer)?;

        // If no `getheaders` request was sent to the peer, the `headers` message is unsolicited.
        // Don't accept more than a few headers in that case.
        if headers.len() > MAX_UNSOLICITED_HEADERS && peer.last_asked.is_none() {
            return Err(ReceivedHeadersMessageError::ReceivedTooManyUnsolicitedHeaders);
        }

        // There are more than 2000 headers in the `headers` message.
        if headers.len() > MAX_HEADERS_SIZE {
            return Err(ReceivedHeadersMessageError::ReceivedTooManyHeaders);
        }

        // Grab the last header's block hash. If not found, no headers to add so exit early.
        let last_block_hash = match headers.last() {
            Some(header) => header.block_hash(),
            None => return Ok(()),
        };

        let prev_tip_height = self.blockchain.get_active_chain_tip().height;

        let (added_headers, maybe_err) = self.blockchain.add_headers(headers);
        let active_tip = self.blockchain.get_active_chain_tip();
        if prev_tip_height < active_tip.height {
            slog::info!(
                self.logger,
                "Added headers in the headers message. State Changed. Height = {}, Active chain's tip = {}",
                active_tip.height,
                active_tip.header.block_hash()
            );
        }

        // Update the peer's tip and height to the last
        let maybe_last_header = if added_headers.last().is_some() {
            added_headers.last()
        } else if self.blockchain.get_header(&last_block_hash).is_some() {
            self.blockchain.get_header(&last_block_hash)
        } else {
            None
        };

        if let Some(last) = maybe_last_header {
            if last.height > peer.height {
                peer.tip = last.header.block_hash();
                peer.height = last.height;
                slog::debug!(
                    self.logger,
                    "Peer {}'s height = {}, tip = {}",
                    addr,
                    peer.height,
                    peer.tip
                );
            }
        }

        let maybe_locators = match maybe_err {
            Some(AddHeaderError::InvalidHeader(_)) => {
                return Err(ReceivedHeadersMessageError::ReceivedInvalidHeader)
            }
            Some(AddHeaderError::PrevHeaderNotCached(stop_hash)) => {
                Some((self.blockchain.locator_hashes(), stop_hash))
            }
            None => {
                if let Some(last) = maybe_last_header {
                    // If the headers length is less than the max headers size (2000), it is likely that the end
                    // of the chain has been reached.
                    if headers.len() < MAX_HEADERS_SIZE {
                        None
                    } else {
                        Some((vec![last.header.block_hash()], BlockHash::default()))
                    }
                } else {
                    None
                }
            }
        };

        if let Some(locators) = maybe_locators {
            self.send_getheaders(addr, locators, OnTimeout::Ignore);
        } else {
            // If the adapter is not going to ask for more headers, the peer's last_asked should
            // be reset.
            peer.last_asked = None;
        }

        Ok(())
    }

    /// This function processes "block" messages received from Bitcoin nodes
    fn received_block_message(
        &mut self,
        addr: &SocketAddr,
        block: &Block,
    ) -> Result<(), ReceivedBlockMessageError> {
        let peer = self
            .peer_info
            .get_mut(addr)
            .ok_or(ReceivedBlockMessageError::UnknownPeer)?;

        let block_hash = block.block_hash();

        let inv = Inventory::Block(block_hash);
        let maybe_request_info = self.get_data_request_info.get(&inv);
        let time_taken = match maybe_request_info {
            Some(request_info) => request_info
                .sent_at
                .elapsed()
                .unwrap_or_else(|_| Duration::new(0, 0)),
            None => Duration::new(0, 0),
        };

        slog::info!(
            self.logger,
            "Received block message from {} : Took {:?}sec. Block {:?}",
            addr,
            time_taken,
            block_hash
        );

        match self.blockchain.add_block(block.clone()) {
            Ok(block_height) => {
                slog::info!(
                    self.logger,
                    "Block added to the blockchain successfully with height = {}",
                    block_height
                );

                //Remove the corresponding `GetData` request from peer_info and get_data_request_info.
                if let Some(request_info) = maybe_request_info {
                    if request_info.socket == *addr {
                        peer.num_of_outstanding_get_data_requests =
                            peer.num_of_outstanding_get_data_requests.saturating_sub(1);
                    }
                }

                self.get_data_request_info.remove(&inv);
                Ok(())
            }
            Err(err) => {
                slog::warn!(
                    self.logger,
                    "Unable to add the received block in blockchain. Error: {:?}",
                    err
                );
                Err(ReceivedBlockMessageError::BlockNotAdded)
            }
        }
    }

    /// This function adds a new peer to `peer_info`
    /// and initiates sync with the peer by sending `getheaders` message.
    fn add_peer(&mut self, addr: &SocketAddr) {
        if self.peer_info.contains_key(addr) {
            return;
        }
        slog::info!(self.logger, "Adding peer_info with addr : {} ", addr);
        let initial_hash = self.blockchain.genesis().header.block_hash();
        self.peer_info.insert(
            *addr,
            PeerInfo {
                socket: *addr,
                height: self.blockchain.genesis().height,
                tip: initial_hash,
                last_asked: None,
                sent_at: None,
                on_timeout: OnTimeout::Ignore,
                num_of_outstanding_get_data_requests: 0,
            },
        );
        let locators = (vec![initial_hash], BlockHash::default());
        self.send_getheaders(addr, locators, OnTimeout::Disconnect);
    }

    /// This function adds a new peer to `peer_info`
    /// and initiates sync with the peer by sending `getheaders` message.
    pub fn remove_peer(&mut self, addr: &SocketAddr) {
        slog::info!(self.logger, "Removing peer_info with addr : {} ", addr);
        self.peer_info.remove(addr);
        // Removing all the `GetData` requests that have been sent to the peer before.
        self.get_data_request_info.retain(|_, v| v.socket != *addr);
    }

    fn filter_expired_get_data_requests(&mut self) {
        let now = SystemTime::now();
        let timeout_period = Duration::new(GETDATA_REQUEST_TIMEOUT_SECS, 0);
        let mut requests_to_remove = vec![];
        for request in self.get_data_request_info.values_mut() {
            if request.sent_at + timeout_period < now {
                if let Some(peer) = self.peer_info.get_mut(&request.socket) {
                    peer.num_of_outstanding_get_data_requests =
                        peer.num_of_outstanding_get_data_requests.saturating_sub(1);
                }
                requests_to_remove.push(request.inventory);
            }
        }

        for entry in requests_to_remove {
            self.get_data_request_info.remove(&entry);
        }
    }

    pub fn sync_blocks(&mut self) {
        if self.inventory_to_be_synced.is_empty() {
            return;
        }

        slog::info!(
            self.logger,
            "Syning blocks. Inventory to be synced : {:?}",
            self.inventory_to_be_synced
        );

        // Removing expired GetData requests from `self.get_data_request_info`
        self.filter_expired_get_data_requests();

        // Filter out the inventory for which GetData request has already been sent and the request hasn't timed out yet.
        // We will send GetData requests only for the inventory which hasn't been request before, or for which the earlier request has expired.
        let mut inventory_to_be_synced =
            &self.inventory_to_be_synced - &self.get_data_request_info.keys().copied().collect();
        slog::info!(self.logger, "Syning blocks. Inventory to be synced after filtering out the past GetData requests : {:?}", inventory_to_be_synced);

        // PeerInfo for each peer stores the `num_of_outstanding_get_data_requests`
        // We prefer to send GetData requests to those peers for which `num_of_outstanding_get_data_requests` is lowest.
        // We thereby sort the peers in descending order based on this metric.
        let mut peer_info: Vec<_> = self.peer_info.values_mut().collect();
        peer_info.sort_by(|a, b| {
            a.num_of_outstanding_get_data_requests
                .cmp(&b.num_of_outstanding_get_data_requests)
        });

        slog::debug!(
            self.logger,
            "List of Bitcoin peers: {:?}",
            peer_info
                .iter()
                .map(|p| p.socket)
                .collect::<Vec<SocketAddr>>(),
        );
        slog::info!(
            self.logger,
            "Number of outstanding getdata requests : {:?}",
            peer_info
                .iter()
                .map(|peer| peer.num_of_outstanding_get_data_requests)
                .collect::<Vec<u32>>()
        );

        // For each peer, select a random subset of the inventory and send a "GetData" request for it.
        for peer in peer_info {
            // Calculate number of inventory that can be sent in 'GetData' request to the peer.
            let num_requests_to_be_sent =
                INV_PER_GET_DATA_REQUEST.saturating_sub(peer.num_of_outstanding_get_data_requests);

            // Randomly sample some inventory to be requested from the peer.
            let selected_inventory = inventory_to_be_synced
                .iter()
                .cloned()
                .choose_multiple(&mut self.rng, num_requests_to_be_sent as usize);

            if selected_inventory.is_empty() {
                break;
            }

            slog::info!(
                self.logger,
                "Sending GetData to {} : Inventory {:?}",
                peer.socket,
                selected_inventory
            );

            //Send 'GetData' request for the inventory to the peer.
            self.outgoing_command_queue.push(Command {
                address: Some(peer.socket),
                message: NetworkMessage::GetData(selected_inventory.clone()),
            });

            peer.num_of_outstanding_get_data_requests = peer
                .num_of_outstanding_get_data_requests
                .saturating_add(selected_inventory.len() as u32);
            for inv in selected_inventory {
                // Record the `getdata` request.
                self.get_data_request_info.insert(
                    inv,
                    GetDataRequestInfo {
                        socket: peer.socket,
                        inventory: inv,
                        sent_at: SystemTime::now(),
                        on_timeout: OnTimeout::Ignore,
                    },
                );

                // Remove the inventory that is going to be sent.
                inventory_to_be_synced.remove(&inv);
            }
        }

        self.inventory_to_be_synced = inventory_to_be_synced;
    }

    /// This function is called by the adapter when a new event takes place.
    /// The event could be receiving "GetHeaders", "GetData", "Inv" messages from bitcion peers.
    /// The event could be change in connection status with a bitcoin peer.
    pub fn process_event(&mut self, event: &StreamEvent) -> Result<(), ProcessEventError> {
        if let StreamEventKind::Message(message) = &event.kind {
            match message {
                NetworkMessage::Inv(inventory) => {
                    if self
                        .received_inv_message(&event.address, inventory)
                        .is_err()
                    {
                        return Err(ProcessEventError::InvalidMessage);
                    }
                }
                NetworkMessage::Headers(headers) => {
                    if self
                        .received_headers_message(&event.address, headers)
                        .is_err()
                    {
                        return Err(ProcessEventError::InvalidMessage);
                    }
                }
                NetworkMessage::Block(block) => {
                    if self.received_block_message(&event.address, block).is_err() {
                        return Err(ProcessEventError::InvalidMessage);
                    }
                }
                _ => {}
            };
        }
        Ok(())
    }

    /// This heartbeat method is called periodically by the adapter.
    /// This method is used to send messages to Bitcoin peers.
    pub fn tick(&mut self, channel: &mut impl Channel) {
        // Update the list of peers.
        let active_connections = channel.available_connections();
        // Removing inactive peers.
        let peer_addresses: Vec<SocketAddr> =
            self.peer_info.iter().map(|(addr, _)| *addr).collect();

        for addr in peer_addresses {
            if !active_connections.contains(&addr) {
                self.remove_peer(&addr);
            }
        }

        // Add new active peers.
        for addr in active_connections {
            if !self.peer_info.contains_key(&addr) {
                self.add_peer(&addr);
            }
        }

        self.sync_blocks();
        for command in self.outgoing_command_queue.iter() {
            //TODO: Is it alright to use ".ok()" here? Will it ever cause the code to panic?
            channel.send(command.clone()).ok();
        }
        self.outgoing_command_queue = vec![];
    }

    // TODO: ER-1943: Implement "smart adapters" which prefer to return blocks in the longest chain.
    /// This method returns the list of all successors (of at most given depth) to the given list of block hashes.
    /// If depth = 1, the method returns immediate successors of `block_hashes`.
    /// If depth = 2, the method returns immediate successors of `block_hashes`, and immediate successors of the immediate successors.
    ///                               | -> 2'
    /// Example: if the chain is 0 -> 1 -> 2 -> 3 -> 4 -> 5 and the block hashes received are {1, 2, 3} with a depth of 1, then {2', 4} is returned.
    fn get_successor_block_hashes(
        &self,
        block_hashes: &HashSet<BlockHash>,
        mut depth: u32,
    ) -> HashSet<BlockHash> {
        if depth < 1 {
            depth = 1;
        }
        let mut result: HashSet<BlockHash> = block_hashes.clone();

        for _i in 0..depth {
            let successors: HashSet<BlockHash> = result
                .iter()
                .filter_map(|block_hash| self.blockchain.get_children(block_hash))
                .flatten()
                .cloned()
                .collect();
            result.extend(&successors);
        }
        &result - block_hashes
    }
}

impl HandleClientRequest for BlockchainManager {
    // TODO: ER-2124: BlockchainManager should only provide blocks when fully synced.
    /// This method is called by Blockmananger::process_event when connection status with a Bitcoin node changed.
    /// If a node is disconnected, this method will remove the peer's info inside BlockChainManager.
    /// If a node is added to active peers list, this method will add the peer's info inside BlockChainManager.
    fn handle_client_request(&mut self, block_hashes: Vec<BlockHash>) -> Option<&Block> {
        slog::info!(
            self.logger,
            "Received a request for following block hashes from system component : {:?}",
            block_hashes
        );
        let block_hashes_set: HashSet<BlockHash> = block_hashes.iter().cloned().collect();
        // Compute the entire set of block hashes that are immediate successors of the input `block_hashes`.
        let immediate_successor_block_hashes: HashSet<BlockHash> =
            self.get_successor_block_hashes(&block_hashes_set, 1);
        // Compute the next 5 levels of successor block hashes of the input `block_hashes`.
        let mut future_successor_block_hashes: HashSet<BlockHash> =
            self.get_successor_block_hashes(&block_hashes_set, FUTURE_SUCCESSORS_DEPTH);
        slog::info!(
            self.logger,
            "Successor block hashes : {:?}, Future successor block hashes : {:?}",
            immediate_successor_block_hashes,
            future_successor_block_hashes
        );

        //Prune old blocks from block_cache.
        self.blockchain.prune_old_blocks(&block_hashes);

        // Fetch the blockchain state that contain blocks corresponding to the `immediate_successor_block_hashes`.
        let mut successor_blocks = vec![];
        for hash in &immediate_successor_block_hashes {
            if let Some(block) = self.blockchain.get_block(hash) {
                successor_blocks.push(block);
            }
        }

        // Remove the found successor block hashes from `future_successor_block_hashes`.
        // The future successor block hashes will be used to send `GetData` requests so blocks may be cached
        // prior to being requested.
        for successor in &successor_blocks {
            future_successor_block_hashes.remove(&successor.block_hash());
        }

        slog::info!(
            self.logger,
            "Number of blocks cached: {}, Number of uncached successor blocks : {}",
            successor_blocks.len(),
            future_successor_block_hashes.len()
        );

        //Add `uncached_successor_block_hashes` to `self.inventory_to_be_synced`
        //so as to send GetData requests in the future.
        for block_hash in future_successor_block_hashes {
            self.inventory_to_be_synced
                .insert(Inventory::Block(block_hash));
        }

        successor_blocks.get(0).cloned()
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::common::test_common::{
        generate_headers, make_logger, TestState, BLOCK_1_ENCODED, BLOCK_2_ENCODED,
    };
    use crate::config::test::ConfigBuilder;
    use crate::config::Config;
    use bitcoin::consensus::deserialize;
    use bitcoin::{
        network::message::NetworkMessage, network::message_blockdata::Inventory, BlockHash,
    };
    use hex::FromHex;
    use std::net::SocketAddr;
    use std::str::FromStr;

    /// Tests `BlockchainManager::send_getheaders(...)` to ensure the manager's outgoing command
    /// queue
    #[test]
    fn test_manager_can_send_getheaders_messages() {
        let config = ConfigBuilder::new().build();
        let mut blockchain_manager = BlockchainManager::new(&config, make_logger());
        let addr = SocketAddr::from_str("127.0.0.1:8333").expect("bad address format");
        blockchain_manager.add_peer(&addr);
        assert_eq!(blockchain_manager.outgoing_command_queue.len(), 1);
        let genesis_hash = blockchain_manager.blockchain.genesis().header.block_hash();

        let locators = (vec![genesis_hash], BlockHash::default());
        blockchain_manager.send_getheaders(&addr, locators, OnTimeout::Disconnect);

        let command = blockchain_manager
            .outgoing_command_queue
            .get(0)
            .expect("command not found");
        assert!(matches!(command.address, Some(address) if address == addr));
        assert!(
            matches!(&command.message, NetworkMessage::GetHeaders(GetHeadersMessage { version: _, locator_hashes: _, stop_hash }) if *stop_hash == BlockHash::default())
        );
        assert!(
            matches!(&command.message, NetworkMessage::GetHeaders(GetHeadersMessage { version, locator_hashes: _, stop_hash: _ }) if *version == MINIMUM_VERSION_NUMBER)
        );
        assert!(
            matches!(&command.message, NetworkMessage::GetHeaders(GetHeadersMessage { version: _, locator_hashes, stop_hash: _ }) if locator_hashes[0] == genesis_hash)
        );

        // Check peer info to ensure it has been updated.
        let peer_info = blockchain_manager
            .peer_info
            .get(&addr)
            .expect("peer missing");
        let locators = peer_info
            .last_asked
            .clone()
            .expect("last asked should contain locators");
        assert_eq!(locators.0.len(), 1);
        assert_eq!(
            *locators.0.first().expect("there should be 1 locator"),
            genesis_hash
        );
    }

    /// This unit test is used to verify if the BlockChainManager initiates sync from `adapter_genesis_hash`
    /// whenever a new peer is added.
    /// The test creates a new blockchain manager, an aribtrary chain and 3 peers.
    /// The test then adds each of the peers and verifies the response from the blockchain manager.
    #[test]
    fn test_init_sync() {
        let config = Config::default();
        let mut blockchain_manager = BlockchainManager::new(&config, make_logger());

        // Create an arbitrary chain and adding to the BlockchainState.
        let chain = generate_headers(
            blockchain_manager.blockchain.genesis().header.block_hash(),
            blockchain_manager.blockchain.genesis().header.time,
            16,
        );
        let chain_hashes: Vec<BlockHash> = chain.iter().map(|header| header.block_hash()).collect();
        println!("Adding block hashes {:?}", chain_hashes);

        let runtime = tokio::runtime::Runtime::new().expect("runtime err");

        let sockets = vec![
            SocketAddr::from_str("127.0.0.1:8333").expect("bad address format"),
            SocketAddr::from_str("127.0.0.1:8334").expect("bad address format"),
            SocketAddr::from_str("127.0.0.1:8335").expect("bad address format"),
        ];
        runtime.block_on(async {
            for socket in sockets.iter() {
                blockchain_manager.add_peer(socket);
                if let Some(command) = blockchain_manager.outgoing_command_queue.first() {
                    assert_eq!(
                        command.address.unwrap(),
                        *socket,
                        "The GetHeaders command is not for the added peer"
                    );
                    assert!(
                        matches!(command.message, NetworkMessage::GetHeaders(_)),
                        "Didn't send GetHeaders command after adding the peer"
                    );
                    if let NetworkMessage::GetHeaders(get_headers_message) = &command.message {
                        assert_eq!(
                            get_headers_message.locator_hashes,
                            vec![blockchain_manager.blockchain.genesis().header.block_hash()],
                            "Didn't send the right genesis hash for initial syncing"
                        );
                        assert_eq!(
                            get_headers_message.stop_hash,
                            BlockHash::default(),
                            "Didn't send the right stop hash for initial syncing"
                        );
                    }

                    let event = StreamEvent {
                        address: *socket,
                        kind: StreamEventKind::Message(NetworkMessage::Headers(chain.clone())),
                    };

                    assert!(blockchain_manager.process_event(&event).is_ok());
                    let peer = blockchain_manager.peer_info.get(socket).unwrap();
                    assert_eq!(peer.height, 17, "Height of peer {} is not correct", socket);
                    assert_eq!(
                        blockchain_manager.blockchain.get_active_chain_tip().height,
                        17,
                        "Height of the blockchain is not matching after adding the headers"
                    );
                    blockchain_manager.outgoing_command_queue.remove(0);
                } else {
                    panic!("No command sent after adding a peer");
                }
            }
        });
    }

    #[test]
    /// This unit test verifies if the incoming inv messages are processed correctly.
    /// This test first creates a BlockChainManager, adds a peer, and let the initial sync happen.
    /// The test then sends an inv message for a fork chain, and verifies if the BlockChainManager responds correctly.
    fn test_received_inv() {
        let config = Config::default();
        let mut blockchain_manager = BlockchainManager::new(&config, make_logger());

        // Create an arbitrary chain and adding to the BlockchainState.
        let chain = generate_headers(
            blockchain_manager.blockchain.genesis().header.block_hash(),
            blockchain_manager.blockchain.genesis().header.time,
            16,
        );
        let chain_hashes: Vec<BlockHash> = chain.iter().map(|header| header.block_hash()).collect();
        println!("Adding block hashes {:?}", chain_hashes);

        let runtime = tokio::runtime::Runtime::new().expect("runtime err");

        let sockets = vec![
            SocketAddr::from_str("127.0.0.1:8333").expect("bad address format"),
            SocketAddr::from_str("127.0.0.1:8334").expect("bad address format"),
            SocketAddr::from_str("127.0.0.1:8335").expect("bad address format"),
        ];
        runtime.block_on(async {
            blockchain_manager.add_peer(&sockets[0]);
            blockchain_manager.outgoing_command_queue.remove(0);
            let event = StreamEvent {
                address: sockets[0],
                kind: StreamEventKind::Message(NetworkMessage::Headers(chain.clone())),
            };
            assert!(blockchain_manager.process_event(&event).is_ok());

            assert_eq!(
                blockchain_manager.blockchain.get_active_chain_tip().height,
                17,
                "Height of the blockchain is not matching after adding the headers"
            );

            //Send an inv message for a fork chain.
            let fork_chain = generate_headers(chain_hashes[10], chain[10].time, 16);
            let fork_hashes: Vec<BlockHash> = fork_chain
                .iter()
                .map(|header| header.block_hash())
                .collect();
            let message = NetworkMessage::Inv(
                fork_hashes
                    .iter()
                    .map(|hash| Inventory::Block(*hash))
                    .collect(),
            );
            let event = StreamEvent {
                address: sockets[0],
                kind: StreamEventKind::Message(message),
            };
            assert!(blockchain_manager.process_event(&event).is_ok());
            blockchain_manager.add_peer(&sockets[0]);
            if let Some(command) = blockchain_manager.outgoing_command_queue.first() {
                assert_eq!(
                    command.address.unwrap(),
                    sockets[0],
                    "The GetHeaders command is not for the correct peer"
                );
                assert!(
                    matches!(command.message, NetworkMessage::GetHeaders(_)),
                    "Didn't send GetHeaders command in response to inv message"
                );
                if let NetworkMessage::GetHeaders(get_headers_message) = &command.message {
                    assert!(
                        !get_headers_message.locator_hashes.is_empty(),
                        "Sent 0 locator hashes in GetHeaders message"
                    );
                    assert_eq!(
                        get_headers_message.locator_hashes.first().unwrap(),
                        chain_hashes.last().unwrap(),
                        "Didn't send the right locator hashes in response to inv message"
                    );
                    assert_eq!(
                        *get_headers_message.locator_hashes.last().unwrap(),
                        blockchain_manager.blockchain.genesis().header.block_hash(),
                        "Didn't send the right locator hashes in response to inv message"
                    );
                    assert_eq!(
                        get_headers_message.stop_hash,
                        *fork_hashes.last().unwrap(),
                        "Didn't send the right stop hash when responding to inv message"
                    );
                }
                blockchain_manager.outgoing_command_queue.remove(0);
            } else {
                panic!("The BlockChainManager didn't respond to inv message");
            }
        });
    }

    /// This test performs a surface level check to make ensure the `sync_blocks` and `received_block_message`
    /// increment and decrement the `num_of_outstanding_get_data_requests`.
    #[test]
    fn test_simple_sync_blocks_and_received_block_message_lifecycle() {
        let peer_addr = SocketAddr::from_str("127.0.0.1:8333").expect("bad address format");
        // Mainnet block 00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048
        let encoded_block_1 = Vec::from_hex(BLOCK_1_ENCODED).expect("unable to make vec from hex");
        // Mainnet block 000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd
        let encoded_block_2 = Vec::from_hex(BLOCK_2_ENCODED).expect("unable to make vec from hex");
        let block_1: Block = deserialize(&encoded_block_1).expect("failed to decoded block 1");
        let block_2: Block = deserialize(&encoded_block_2).expect("failed to decoded block 2");

        let config = Config::default();
        let mut blockchain_manager = BlockchainManager::new(&config, make_logger());
        let headers = vec![block_1.header, block_2.header];
        // Initialize the blockchain manager state
        let (added_headers, maybe_err) = blockchain_manager.blockchain.add_headers(&headers);
        assert_eq!(added_headers.len(), headers.len());
        assert!(maybe_err.is_none());
        blockchain_manager
            .inventory_to_be_synced
            .insert(Inventory::Block(block_1.block_hash()));
        blockchain_manager
            .inventory_to_be_synced
            .insert(Inventory::Block(block_2.block_hash()));

        blockchain_manager.add_peer(&peer_addr);
        // Ensure that the number of requests is at 0.
        {
            let peer_info = blockchain_manager
                .peer_info
                .get(&peer_addr)
                .expect("peer should be here");

            assert_eq!(peer_info.num_of_outstanding_get_data_requests, 0);
        }

        // Sync block information.
        blockchain_manager.sync_blocks();
        // Ensure there are now 2 outbound requests for the blocks.
        {
            let peer_info = blockchain_manager
                .peer_info
                .get(&peer_addr)
                .expect("peer should be here");
            assert_eq!(peer_info.num_of_outstanding_get_data_requests, 2);
        }

        // Ensure there is now 1 request.
        let result = blockchain_manager.received_block_message(&peer_addr, &block_1);
        assert!(result.is_ok());
        {
            let peer_info = blockchain_manager
                .peer_info
                .get(&peer_addr)
                .expect("peer should be here");
            assert_eq!(peer_info.num_of_outstanding_get_data_requests, 1);
        }

        let result = blockchain_manager.received_block_message(&peer_addr, &block_2);
        assert!(result.is_ok());
        blockchain_manager.sync_blocks();
        // Ensure there is now zero requests.
        {
            let peer_info = blockchain_manager
                .peer_info
                .get(&peer_addr)
                .expect("peer should be here");
            assert_eq!(peer_info.num_of_outstanding_get_data_requests, 0);
        }
    }

    #[test]
    fn test_get_successor_block_hashes() {
        let test_state = TestState::setup();
        let config = ConfigBuilder::new().build();
        let mut blockchain_manager = BlockchainManager::new(&config, make_logger());

        // Set up the following chain:
        // |-> 1' -> 2'
        // 0 -> 1 -> 2
        let main_chain = vec![test_state.block_1.header, test_state.block_2.header];
        let side_chain = generate_headers(
            blockchain_manager.blockchain.genesis().header.block_hash(),
            blockchain_manager.blockchain.genesis().header.time,
            2,
        );
        blockchain_manager.blockchain.add_headers(&main_chain);
        blockchain_manager.blockchain.add_headers(&side_chain);

        let block_hashes = vec![blockchain_manager.blockchain.genesis().header.block_hash()]
            .into_iter()
            .collect();

        //             |-> 1' -> 2'
        // If chain is 0 -> 1 -> 2 and block hashes are {0}  then {1, 1'} should be returned.
        let successor_hashes = blockchain_manager.get_successor_block_hashes(&block_hashes, 1);
        assert_eq!(successor_hashes.len(), 2);
        assert!(successor_hashes.contains(&test_state.block_1.block_hash()));
        assert!(successor_hashes.contains(&side_chain[0].block_hash()));
        //             |-> 1' -> 2'
        // If chain is 0 -> 1 -> 2 and block hashes are {0, 1}  then {1', 2, 2'} should be returned.
        let block_hashes = vec![
            blockchain_manager.blockchain.genesis().header.block_hash(),
            test_state.block_1.block_hash(),
        ]
        .into_iter()
        .collect();
        let successor_hashes = blockchain_manager.get_successor_block_hashes(&block_hashes, 2);

        assert_eq!(successor_hashes.len(), 3);
        assert!(successor_hashes.contains(&side_chain[0].block_hash()));
        assert!(successor_hashes.contains(&side_chain[1].block_hash()));
        assert!(successor_hashes.contains(&test_state.block_2.block_hash()));
    }
}