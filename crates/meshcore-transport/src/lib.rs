//! MeshCore packet definitions and radio transport trait definitions.
//! Includes a mock socket radio transport harness for local loopback testing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("Failed to send packet: {0}")]
    SendError(String),
    #[error("Failed to receive packet: {0}")]
    ReceiveError(String),
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("MTU exceeded: payload was {0} bytes, limit is {1}")]
    MtuExceeded(usize, usize),
}

/// MeshCore packet representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadioPacket {
    pub is_broadcast: bool,
    pub src_node: [u8; 32],
    pub dst_node: [u8; 32],
    pub payload: Vec<u8>,
    pub signal_rssi: i16,
    pub signal_snr: i8,
}

/// Main interface representing a physical or virtual radio transceiver.
#[async_trait]
pub trait RadioTransport: Send + Sync {
    /// Dispatches a packet to the radio.
    async fn send_packet(&self, packet: RadioPacket) -> Result<(), TransportError>;
    
    /// Blocks until an incoming packet is captured.
    async fn receive_packet(&self) -> Result<RadioPacket, TransportError>;
    
    /// Calculates the estimated LoRa airtime in milliseconds for a payload size.
    fn get_estimated_airtime_ms(&self, payload_len: usize) -> u32;
    
    /// Returns the current rolling duty cycle percentage of the transmitter.
    fn get_current_duty_cycle(&self) -> f32;

    /// Returns the Maximum Transmission Unit (MTU) of the transport layer.
    fn get_mtu(&self) -> usize;
}

/// Shared statistics tracker for transport-level packet accounting.
///
/// All counters use relaxed atomic ordering for low-overhead, best-effort
/// accuracy.  The `packet_timestamps` vec is pruned to the last 24 hours
/// whenever [`packets_per_minute_last`] is called.
pub struct TransportStats {
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub send_errors: AtomicU64,
    pub receive_errors: AtomicU64,
    pub started_at: Instant,
    pub packet_timestamps: Mutex<Vec<(Instant, bool)>>,
}

impl TransportStats {
    /// Creates a new stats tracker with all counters zeroed.
    pub fn new() -> Self {
        Self {
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            send_errors: AtomicU64::new(0),
            receive_errors: AtomicU64::new(0),
            started_at: Instant::now(),
            packet_timestamps: Mutex::new(Vec::new()),
        }
    }

    /// Records a successful send of `payload_bytes` bytes.
    pub fn record_send(&self, payload_bytes: usize) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(payload_bytes as u64, Ordering::Relaxed);
        if let Ok(mut ts) = self.packet_timestamps.lock() {
            ts.push((Instant::now(), true));
        }
    }

    /// Records a successful receive of `payload_bytes` bytes.
    pub fn record_receive(&self, payload_bytes: usize) {
        self.packets_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(payload_bytes as u64, Ordering::Relaxed);
        if let Ok(mut ts) = self.packet_timestamps.lock() {
            ts.push((Instant::now(), false));
        }
    }

    /// Records a send error.
    pub fn record_send_error(&self) {
        self.send_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a receive error.
    pub fn record_receive_error(&self) {
        self.receive_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns `(send_ppm, recv_ppm)` over the last `duration_secs` seconds.
    ///
    /// Also prunes timestamps older than 24 hours.
    pub fn packets_per_minute_last(&self, duration_secs: u64) -> (f64, f64) {
        let now = Instant::now();
        let cutoff_24h = now - std::time::Duration::from_secs(86400);
        let window = now - std::time::Duration::from_secs(duration_secs);

        let mut ts = match self.packet_timestamps.lock() {
            Ok(guard) => guard,
            Err(_) => return (0.0, 0.0),
        };

        // Prune entries older than 24 hours
        ts.retain(|&(t, _)| t >= cutoff_24h);

        let mut send_count: u64 = 0;
        let mut recv_count: u64 = 0;
        for &(t, is_send) in ts.iter() {
            if t >= window {
                if is_send {
                    send_count += 1;
                } else {
                    recv_count += 1;
                }
            }
        }

        let minutes = duration_secs as f64 / 60.0;
        if minutes <= 0.0 {
            return (0.0, 0.0);
        }
        (send_count as f64 / minutes, recv_count as f64 / minutes)
    }

    /// Total packets sent since creation.
    pub fn total_packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }

    /// Total packets received since creation.
    pub fn total_packets_received(&self) -> u64 {
        self.packets_received.load(Ordering::Relaxed)
    }

    /// Total payload bytes sent since creation.
    pub fn total_bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    /// Total payload bytes received since creation.
    pub fn total_bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// Seconds elapsed since the stats tracker was created.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

impl Default for TransportStats {
    fn default() -> Self {
        Self::new()
    }
}

/// A simulated virtual socket transport harness using TCP sockets.
/// Simulates packet drops and transport latency to model LoRa networks.
pub struct MockSocketTransport {
    tx: tokio::sync::mpsc::Sender<RadioPacket>,
    rx: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<RadioPacket>>>,
    _tx_in: Option<tokio::sync::mpsc::Sender<RadioPacket>>,
    #[allow(dead_code)]
    packet_loss_rate: f64,
    #[allow(dead_code)]
    latency_ms: u32,
    mtu: usize,
    pub stats: Arc<TransportStats>,
}

impl MockSocketTransport {
    /// Creates a mock transport running entirely in memory (mainly for unit tests).
    pub fn new(packet_loss_rate: f64, latency_ms: u32, mtu: usize) -> Self {
        let (tx, mut rx_out) = tokio::sync::mpsc::channel(100);
        let (tx_in, rx) = tokio::sync::mpsc::channel(100);
        
        // Spawn a dummy task to drain the outbound queue so it doesn't block or error
        tokio::spawn(async move {
            while rx_out.recv().await.is_some() {}
        });

        Self {
            tx,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx)),
            _tx_in: Some(tx_in),
            packet_loss_rate,
            latency_ms,
            mtu,
            stats: Arc::new(TransportStats::new()),
        }
    }

    /// Creates a mock transport that binds to a TCP port as a server.
    pub fn new_server(addr: String, packet_loss_rate: f64, latency_ms: u32, mtu: usize) -> Self {
        let (tx, rx_out) = tokio::sync::mpsc::channel::<RadioPacket>(100);
        let (tx_in, rx) = tokio::sync::mpsc::channel::<RadioPacket>(100);

        let addr_clone = addr.clone();
        let tx_in_clone = tx_in.clone();
        tokio::spawn(async move {
            if let Ok(listener) = tokio::net::TcpListener::bind(&addr_clone).await {
                log::info!("Mock Socket Broker listening on {}", addr_clone);
                loop {
                    if let Ok((socket, peer_addr)) = listener.accept().await {
                        log::info!("Mock Socket Broker accepted connection from {}", peer_addr);
                        // We forward packets in a connection handler loop
                        let _ = handle_socket_connection(socket, rx_out, tx_in_clone.clone()).await;
                        break; // Close after first connection for testing
                    }
                }
            } else {
                log::error!("Mock Socket Broker failed to bind to {}", addr_clone);
            }
        });

        Self {
            tx,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx)),
            _tx_in: Some(tx_in),
            packet_loss_rate,
            latency_ms,
            mtu,
            stats: Arc::new(TransportStats::new()),
        }
    }

    /// Creates a mock transport that connects to a TCP server.
    pub fn new_client(addr: String, packet_loss_rate: f64, latency_ms: u32, mtu: usize) -> Self {
        let (tx, rx_out) = tokio::sync::mpsc::channel::<RadioPacket>(100);
        let (tx_in, rx) = tokio::sync::mpsc::channel::<RadioPacket>(100);

        let addr_clone = addr.clone();
        let tx_in_clone = tx_in.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(socket) = tokio::net::TcpStream::connect(&addr_clone).await {
                    log::info!("Mock Socket connected to {}", addr_clone);
                    let _ = handle_socket_connection(socket, rx_out, tx_in_clone.clone()).await;
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        });

        Self {
            tx,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx)),
            _tx_in: Some(tx_in),
            packet_loss_rate,
            latency_ms,
            mtu,
            stats: Arc::new(TransportStats::new()),
        }
    }
}

async fn handle_socket_connection(
    mut socket: tokio::net::TcpStream,
    mut rx_out: tokio::sync::mpsc::Receiver<RadioPacket>,
    tx_in: tokio::sync::mpsc::Sender<RadioPacket>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    log::debug!("Mock TCP Socket Broker connection handler started");
    let (mut reader, mut writer) = socket.split();

    let write_loop = async {
        while let Some(packet) = rx_out.recv().await {
            log::debug!("TCP write_loop: forwarding packet of len {} over TCP", packet.payload.len());
            let json = serde_json::to_string(&packet)?;
            let bytes = json.as_bytes();
            let len = bytes.len() as u32;
            writer.write_all(&len.to_be_bytes()).await?;
            writer.write_all(bytes).await?;
            writer.flush().await?;
            log::debug!("TCP write_loop: packet successfully flushed to socket");
        }
        log::debug!("TCP write_loop exited");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };

    let read_loop = async {
        let mut len_bytes = [0u8; 4];
        loop {
            if reader.read_exact(&mut len_bytes).await.is_err() {
                log::debug!("TCP read_loop: read_exact length failed (connection closed by peer)");
                break;
            }
            let len = u32::from_be_bytes(len_bytes) as usize;
            log::debug!("TCP read_loop: reading payload of len {}", len);
            let mut buf = vec![0u8; len];
            if reader.read_exact(&mut buf).await.is_err() {
                log::debug!("TCP read_loop: read_exact payload failed");
                break;
            }
            if let Ok(packet) = serde_json::from_slice::<RadioPacket>(&buf) {
                log::debug!("TCP read_loop: dispatching packet to local receiver");
                if tx_in.send(packet).await.is_err() {
                    log::debug!("TCP read_loop: rx channel receiver dropped");
                    break;
                }
            } else {
                log::debug!("TCP read_loop: failed to deserialize RadioPacket");
            }
        }
        log::debug!("TCP read_loop exited");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };

    tokio::select! {
        r = write_loop => r,
        r = read_loop => r,
    }
}

#[async_trait]
impl RadioTransport for MockSocketTransport {
    async fn send_packet(&self, packet: RadioPacket) -> Result<(), TransportError> {
        if packet.payload.len() > self.mtu {
            return Err(TransportError::MtuExceeded(packet.payload.len(), self.mtu));
        }

        // Simulate LoRa transmission latency
        if self.latency_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.latency_ms as u64)).await;
        }

        let payload_len = packet.payload.len();
        let dst = packet.dst_node;
        if self.tx.send(packet).await.is_err() {
            self.stats.record_send_error();
            return Err(TransportError::SendError("Transport channel closed".to_string()));
        }

        self.stats.record_send(payload_len);
        log::info!(
            "[RADIO TX] {} bytes -> node {:02x}{:02x}..{:02x}{:02x}",
            payload_len,
            dst[0], dst[1], dst[30], dst[31],
        );

        Ok(())
    }

    async fn receive_packet(&self) -> Result<RadioPacket, TransportError> {
        let mut rx = self.rx.lock().await;
        if let Some(packet) = rx.recv().await {
            self.stats.record_receive(packet.payload.len());
            log::info!(
                "[RADIO RX] {} bytes <- node {:02x}{:02x}..{:02x}{:02x}",
                packet.payload.len(),
                packet.src_node[0], packet.src_node[1], packet.src_node[30], packet.src_node[31],
            );
            Ok(packet)
        } else {
            Err(TransportError::ConnectionClosed)
        }
    }

    fn get_estimated_airtime_ms(&self, payload_len: usize) -> u32 {
        let bytes_per_sec = 250;
        ((payload_len as f32 / bytes_per_sec as f32) * 1000.0) as u32
    }

    fn get_current_duty_cycle(&self) -> f32 {
        0.02
    }

    fn get_mtu(&self) -> usize {
        self.mtu
    }
}

/// CRC16 calculation helper using standard CRC16-CCITT polynomial (0x1021).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Represents a high-level application message that can be fragmented/reassembled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshBbsMessage {
    pub channel_flag: u8,
    pub opcode: u8,
    pub flags: u8,
    pub payload: Vec<u8>,
}

impl MeshBbsMessage {
    pub fn new(channel_flag: u8, opcode: u8, flags: u8, payload: Vec<u8>) -> Self {
        Self {
            channel_flag,
            opcode,
            flags,
            payload,
        }
    }

    /// Serializes and fragments the message into payloads for RadioPackets.
    pub fn to_fragments(&self, mtu: usize) -> Result<Vec<Vec<u8>>, String> {
        if mtu <= 4 {
            return Err("MTU too small to fit Sub-Header".to_string());
        }
        let chunk_max_size = mtu - 4;

        // Construct Inner Message Buffer: Opcode (1B) | Flags (1B) | Payload Len (1B) | CRC16 (2B) | Payload (N Bytes)
        let mut inner_buf = Vec::with_capacity(5 + self.payload.len());
        inner_buf.push(self.opcode);
        inner_buf.push(self.flags);

        let pay_len = if self.payload.len() > 255 {
            255
        } else {
            self.payload.len() as u8
        };
        inner_buf.push(pay_len);

        let crc = crc16(&self.payload);
        inner_buf.extend_from_slice(&crc.to_be_bytes());
        inner_buf.extend_from_slice(&self.payload);

        // Split inner_buf into chunks
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < inner_buf.len() {
            let end = std::cmp::min(offset + chunk_max_size, inner_buf.len());
            chunks.push(inner_buf[offset..end].to_vec());
            offset = end;
        }

        let total_chunks = chunks.len();
        if total_chunks > 255 {
            return Err("Message too large, exceeds maximum 255 fragments".to_string());
        }

        let mut fragments = Vec::new();
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut frag = Vec::with_capacity(4 + chunk.len());
            frag.push(0xBB); // BBS App Port
            frag.push(self.channel_flag);
            frag.push((i + 1) as u8); // Frame Seq (1-indexed)
            frag.push(total_chunks as u8);
            frag.extend_from_slice(&chunk);
            fragments.push(frag);
        }

        Ok(fragments)
    }

    /// Attempts to parse a reassembled Inner Message Buffer.
    pub fn from_reassembled(channel_flag: u8, data: &[u8]) -> Result<Self, String> {
        if data.len() < 5 {
            return Err("Data too short to parse Message Header".to_string());
        }
        let opcode = data[0];
        let flags = data[1];
        let _payload_len = data[2];
        let crc_expected = u16::from_be_bytes([data[3], data[4]]);
        let payload = data[5..].to_vec();

        // Verify CRC16
        let crc_actual = crc16(&payload);
        if crc_actual != crc_expected {
            return Err(format!("CRC mismatch: expected {:04X}, got {:04X}", crc_expected, crc_actual));
        }

        Ok(Self {
            channel_flag,
            opcode,
            flags,
            payload,
        })
    }
}

/// Helper to handle stream reassembly of incoming packets per node session.
#[derive(Default)]
pub struct MessageReassembler {
    sessions: std::collections::HashMap<([u8; 32], u8), (u8, std::collections::HashMap<u8, Vec<u8>>)>,
}

impl MessageReassembler {
    pub fn new() -> Self {
        Self {
            sessions: std::collections::HashMap::new(),
        }
    }

    /// Process an incoming fragment. If a message is fully reassembled, returns it.
    pub fn process_packet(&mut self, src_node: [u8; 32], packet_payload: &[u8]) -> Result<Option<MeshBbsMessage>, String> {
        if packet_payload.len() < 4 {
            return Ok(None);
        }
        if packet_payload[0] != 0xBB {
            return Ok(None);
        }

        let channel_flag = packet_payload[1];
        let frame_seq = packet_payload[2];
        let total_chunks = packet_payload[3];
        let chunk_data = &packet_payload[4..];

        if total_chunks == 1 {
            let msg = MeshBbsMessage::from_reassembled(channel_flag, chunk_data)?;
            return Ok(Some(msg));
        }

        let key = (src_node, channel_flag);
        let entry = self.sessions.entry(key).or_insert_with(|| (total_chunks, std::collections::HashMap::new()));

        if entry.0 != total_chunks {
            entry.0 = total_chunks;
            entry.1.clear();
        }

        entry.1.insert(frame_seq, chunk_data.to_vec());

        if entry.1.len() as u8 == total_chunks {
            let mut full_buf = Vec::new();
            for seq in 1..=total_chunks {
                if let Some(chunk) = entry.1.get(&seq) {
                    full_buf.extend_from_slice(chunk);
                } else {
                    return Err(format!("Missing chunk {} during reassembly", seq));
                }
            }
            self.sessions.remove(&(src_node, channel_flag));
            let msg = MeshBbsMessage::from_reassembled(channel_flag, &full_buf)?;
            Ok(Some(msg))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_serialization() {
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: [1; 32],
            dst_node: [2; 32],
            payload: vec![10, 20, 30],
            signal_rssi: -85,
            signal_snr: 8,
        };

        let serialized = serde_json::to_string(&packet).unwrap();
        let deserialized: RadioPacket = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.is_broadcast, packet.is_broadcast);
        assert_eq!(deserialized.src_node, packet.src_node);
        assert_eq!(deserialized.dst_node, packet.dst_node);
        assert_eq!(deserialized.payload, packet.payload);
        assert_eq!(deserialized.signal_rssi, packet.signal_rssi);
        assert_eq!(deserialized.signal_snr, packet.signal_snr);
    }

    #[tokio::test]
    async fn test_mock_transport_send_success() {
        let transport = MockSocketTransport::new(0.0, 5, 200);
        let packet = RadioPacket {
            is_broadcast: true,
            src_node: [0; 32],
            dst_node: [0; 32],
            payload: vec![0; 50],
            signal_rssi: 0,
            signal_snr: 0,
        };

        let result = transport.send_packet(packet).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_transport_send_mtu_exceeded() {
        let transport = MockSocketTransport::new(0.0, 0, 50);
        let packet = RadioPacket {
            is_broadcast: true,
            src_node: [0; 32],
            dst_node: [0; 32],
            payload: vec![0; 51], // 51 bytes exceeds 50 MTU
            signal_rssi: 0,
            signal_snr: 0,
        };

        let result = transport.send_packet(packet).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::MtuExceeded(payload_len, mtu) => {
                assert_eq!(payload_len, 51);
                assert_eq!(mtu, 50);
            }
            _ => panic!("Expected MtuExceeded error"),
        }
    }

    #[tokio::test]
    async fn test_airtime_and_duty_cycle() {
        let transport = MockSocketTransport::new(0.0, 0, 100);
        assert_eq!(transport.get_estimated_airtime_ms(100), 400); // 100 / 250 * 1000 = 400ms
        assert_eq!(transport.get_current_duty_cycle(), 0.02);
    }

    #[test]
    fn test_transport_error_display() {
        assert_eq!(
            TransportError::SendError("timeout".to_string()).to_string(),
            "Failed to send packet: timeout"
        );
        assert_eq!(
            TransportError::ReceiveError("crc mismatch".to_string()).to_string(),
            "Failed to receive packet: crc mismatch"
        );
        assert_eq!(
            TransportError::ConnectionClosed.to_string(),
            "Connection closed"
        );
        assert_eq!(
            TransportError::MtuExceeded(250, 200).to_string(),
            "MTU exceeded: payload was 250 bytes, limit is 200"
        );
    }

    #[tokio::test]
    async fn test_mock_transport_receive_timeout() {
        let transport = MockSocketTransport::new(0.0, 0, 100);
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(5), transport.receive_packet()).await;
        assert!(result.is_err()); // Expect timeout to occur
    }

    #[tokio::test]
    async fn test_tcp_transport_loopback() {
        let bind_addr = "127.0.0.1:9099".to_string();
        
        let server = MockSocketTransport::new_server(bind_addr.clone(), 0.0, 0, 200);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client = MockSocketTransport::new_client(bind_addr, 0.0, 0, 200);
        
        let test_packet = RadioPacket {
            is_broadcast: false,
            src_node: [3; 32],
            dst_node: [4; 32],
            payload: vec![1, 2, 3, 4],
            signal_rssi: -40,
            signal_snr: 12,
        };
        
        let mut sent = false;
        for _ in 0..10 {
            if client.send_packet(test_packet.clone()).await.is_ok() {
                sent = true;
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        assert!(sent, "Failed to send packet from client");
        
        let rx_result = tokio::time::timeout(tokio::time::Duration::from_millis(500), server.receive_packet()).await;
        assert!(rx_result.is_ok(), "Server packet receive timed out");
        let rx_packet = rx_result.unwrap().expect("Failed to receive packet on server");
        assert_eq!(rx_packet.payload, vec![1, 2, 3, 4]);
        
        let response_packet = RadioPacket {
            is_broadcast: false,
            src_node: [4; 32],
            dst_node: [3; 32],
            payload: vec![9, 8, 7],
            signal_rssi: -40,
            signal_snr: 12,
        };
        
        assert!(server.send_packet(response_packet).await.is_ok());
        
        let rx_client_result = tokio::time::timeout(tokio::time::Duration::from_millis(500), client.receive_packet()).await;
        assert!(rx_client_result.is_ok(), "Client packet receive timed out");
        let rx_client_packet = rx_client_result.unwrap().expect("Failed to receive packet on client");
        assert_eq!(rx_client_packet.payload, vec![9, 8, 7]);
    }

    #[test]
    fn test_crc16_correctness() {
        let data = b"123456789";
        // Standard test vector for CRC16-CCITT with polynomial 0x1021, seed 0xFFFF is 0x29B1
        assert_eq!(crc16(data), 0x29B1);
    }

    #[test]
    fn test_message_fragmentation_and_reassembly() {
        let original_payload = vec![0x41; 300]; // 300 bytes of 'A'
        let original_msg = MeshBbsMessage::new(0x01, 0x03, 0x00, original_payload.clone());

        // Fragment with MTU of 100 bytes
        // Each chunk can hold at most 100 - 4 = 96 bytes.
        // Inner buffer size = 5 (headers) + 300 (payload) = 305 bytes.
        // Chunks needed: ceil(305 / 96) = 4 chunks.
        let fragments = original_msg.to_fragments(100).expect("Failed to fragment");
        assert_eq!(fragments.len(), 4);

        for (i, frag) in fragments.iter().enumerate() {
            assert!(frag.len() <= 100);
            assert_eq!(frag[0], 0xBB);
            assert_eq!(frag[1], 0x01); // channel_flag
            assert_eq!(frag[2], (i + 1) as u8); // frame_seq
            assert_eq!(frag[3], 4); // total_chunks
        }

        // Reassemble fragments
        let mut reassembler = MessageReassembler::new();
        let src_node = [7u8; 32];

        // Process fragments 1 to 3
        for i in 0..3 {
            let res = reassembler.process_packet(src_node, &fragments[i]).unwrap();
            assert!(res.is_none(), "Should not be fully reassembled yet");
        }

        // Process final fragment
        let final_res = reassembler.process_packet(src_node, &fragments[3]).unwrap();
        assert!(final_res.is_some(), "Should be fully reassembled now");
        
        let assembled_msg = final_res.unwrap();
        assert_eq!(assembled_msg.channel_flag, 0x01);
        assert_eq!(assembled_msg.opcode, 0x03);
        assert_eq!(assembled_msg.flags, 0x00);
        assert_eq!(assembled_msg.payload, original_payload);
    }

    #[test]
    fn test_message_reassembly_unfragmented() {
        let original_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, vec![1, 2, 3]);
        let fragments = original_msg.to_fragments(100).unwrap();
        assert_eq!(fragments.len(), 1);

        let mut reassembler = MessageReassembler::new();
        let src_node = [7u8; 32];
        let res = reassembler.process_packet(src_node, &fragments[0]).unwrap();
        assert!(res.is_some());
        
        let assembled = res.unwrap();
        assert_eq!(assembled.opcode, 0x02);
        assert_eq!(assembled.payload, vec![1, 2, 3]);
    }

    #[test]
    fn test_message_error_handling() {
        let msg = MeshBbsMessage::new(0x01, 0x01, 0x00, vec![1, 2]);
        assert!(msg.to_fragments(3).is_err(), "Should err with MTU <= 4");

        let mut reassembler = MessageReassembler::new();
        let src = [0u8; 32];
        
        // Too short payload
        assert!(reassembler.process_packet(src, &[0xBB, 1, 1]).unwrap().is_none());
        // Wrong app port
        assert!(reassembler.process_packet(src, &[0xAA, 1, 1, 1, 1]).unwrap().is_none());

        // CRC Mismatch
        let mut corrupted_frag = msg.to_fragments(100).unwrap()[0].clone();
        let len = corrupted_frag.len();
        corrupted_frag[len - 1] ^= 0xFF; // flip bits in payload
        assert!(reassembler.process_packet(src, &corrupted_frag).is_err());
    }

    #[test]
    fn test_transport_stats_new() {
        let stats = TransportStats::new();
        assert_eq!(stats.total_packets_sent(), 0);
        assert_eq!(stats.total_packets_received(), 0);
        assert_eq!(stats.total_bytes_sent(), 0);
        assert_eq!(stats.total_bytes_received(), 0);
        assert_eq!(stats.send_errors.load(Ordering::Relaxed), 0);
        assert_eq!(stats.receive_errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_transport_stats_record_send_receive() {
        let stats = TransportStats::new();
        stats.record_send(100);
        stats.record_send(200);
        stats.record_receive(50);
        stats.record_receive(75);
        stats.record_receive(25);

        assert_eq!(stats.total_packets_sent(), 2);
        assert_eq!(stats.total_packets_received(), 3);
        assert_eq!(stats.total_bytes_sent(), 300);
        assert_eq!(stats.total_bytes_received(), 150);

        stats.record_send_error();
        stats.record_send_error();
        stats.record_receive_error();
        assert_eq!(stats.send_errors.load(Ordering::Relaxed), 2);
        assert_eq!(stats.receive_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_transport_stats_packets_per_minute() {
        let stats = TransportStats::new();
        // Record 6 sends and 3 receives
        for _ in 0..6 {
            stats.record_send(10);
        }
        for _ in 0..3 {
            stats.record_receive(10);
        }

        // All timestamps are within the last 60 seconds
        let (send_ppm, recv_ppm) = stats.packets_per_minute_last(60);
        // 6 sends in 1 minute = 6 ppm
        assert!((send_ppm - 6.0).abs() < 0.01, "send_ppm was {}", send_ppm);
        // 3 receives in 1 minute = 3 ppm
        assert!((recv_ppm - 3.0).abs() < 0.01, "recv_ppm was {}", recv_ppm);

        // With a 120-second window, same counts spread over 2 minutes
        let (send_ppm_2, recv_ppm_2) = stats.packets_per_minute_last(120);
        assert!((send_ppm_2 - 3.0).abs() < 0.01, "send_ppm_2 was {}", send_ppm_2);
        assert!((recv_ppm_2 - 1.5).abs() < 0.01, "recv_ppm_2 was {}", recv_ppm_2);
    }

    #[test]
    fn test_transport_stats_uptime() {
        let stats = TransportStats::new();
        // Just-created stats should have started_at in the past (or equal to now)
        // uptime_secs may be 0 if the test runs fast, but elapsed should be >= 0
        std::thread::sleep(std::time::Duration::from_millis(10));
        // started_at.elapsed() should be > 0 in nanoseconds at least
        assert!(stats.started_at.elapsed().as_nanos() > 0);
    }
}
