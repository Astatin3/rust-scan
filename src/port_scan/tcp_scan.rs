use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use indicatif::ProgressBar;
use pnet::datalink::linux::interfaces;
use pnet::datalink::{self};
use pnet::packet::ip::IpNextHeaderProtocols;
use pnet::packet::tcp::{MutableTcpPacket, TcpFlags, TcpPacket};
use pnet::packet::{tcp, Packet};
use pnet::transport::{self, TransportChannelType, TransportSender};
use rand::random_range;

use super::port_scan::ScanResult;

fn std_to_pnet_ipv4(previous: &IpAddr) -> Ipv4Addr {
    Ipv4Addr::from_str(previous.to_string().as_str()).unwrap()
}

// Main scanning function
pub fn tcp_scan(targets: Vec<IpAddr>, ports: Vec<i32>, timeout: Duration) -> Vec<ScanResult> {
    // Find network interface
    let interfaces = datalink::interfaces();

    println!("{}", interfaces.len());

    let interface = interfaces
        .into_iter()
        .find(|iface| {
            iface.is_up()
                && !iface.is_loopback()
                && !iface.ips.is_empty()
                && !iface.is_dormant()
                && iface.is_running()
                && iface.is_point_to_point()
        })
        .expect("No valid network interface found");

    // Create transport channel for sending and receiving
    let (mut tx, mut rx) = transport::transport_channel(
        65535,
        TransportChannelType::Layer4(pnet::transport::TransportProtocol::Ipv4(
            IpNextHeaderProtocols::Tcp,
        )),
    )
    .expect("Failed to create transport channel");

    // Shared results
    let results = Arc::new(Mutex::new(HashMap::<IpAddr, Vec<i32>>::new()));

    // Initialize results map
    {
        let mut results_map = results.lock().unwrap();
        for ip in &targets {
            results_map.insert(*ip, Vec::new());
        }
    }

    let receiver_results = Arc::clone(&results);
    let receiver_handle = thread::spawn(move || {
        let start_time = std::time::Instant::now();

        while start_time.elapsed() < timeout {
            // println!("loop");
            let mut iter = transport::tcp_packet_iter(&mut rx);

            match iter.next_with_timeout(timeout) {
                Ok(Some((packet, addr))) => {
                    if let Some(tcp) = TcpPacket::new(packet.packet()) {
                        // Check for SYN+ACK flags (indicating open port)
                        if tcp.get_flags() == TcpFlags::SYN | TcpFlags::ACK {
                            println!(
                                "Discovered open port {} on {}",
                                tcp.get_source(),
                                addr.to_string()
                            );
                            let mut results_map = receiver_results.lock().unwrap();
                            if let Some(open_ports) = results_map.get_mut(&addr) {
                                open_ports.push(tcp.get_source() as i32);
                            }
                        }
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(_) => {
                    // Just continue on errors
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    });

    let pb = ProgressBar::new((targets.len() * ports.len()) as u64);

    println!("{:?}", interface.ips);

    let source_ip = interface
        .ips
        .iter()
        .find(|ip| ip.is_ipv4())
        .expect("No IPv4 address found")
        .ip();

    // let source_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    // let source_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 70, 4));

    println!("Using IP: {}", source_ip.to_string());

    for target in &targets {
        for port in &ports {
            // let source_ip = Ipv4Addr::from_bits(random_range(0..=(0xffffffff)));
            let source_port: u16 = random_range(1..=65535);
            // println!("{}", source_ip.to_string());

            let mut tcp_buffer = vec![0u8; 20 + 20]; // IP header + TCP header
            let mut tcp_header = MutableTcpPacket::new(&mut tcp_buffer[0..]).unwrap();

            tcp_header.set_source(source_port);
            tcp_header.set_destination(*port as u16);
            tcp_header.set_sequence(rand::random::<u32>());
            tcp_header.set_acknowledgement(0);
            tcp_header.set_data_offset(5);
            tcp_header.set_reserved(0);
            tcp_header.set_flags(TcpFlags::SYN);
            tcp_header.set_window(64240);
            tcp_header.set_urgent_ptr(0);
            // tcp_header.set_options(&[TcpOption::mss(1460)]);

            // Calculate checksum
            let checksum = tcp::ipv4_checksum(
                &tcp_header.to_immutable(),
                &std_to_pnet_ipv4(&source_ip),
                &std_to_pnet_ipv4(&target),
            );
            tcp_header.set_checksum(checksum);

            send_tcp_packet(&mut tx, tcp_header, target);

            pb.inc(1);
            thread::sleep(Duration::from_micros(100));
        }
    }

    // Wait for receiver to finish
    // thread::sleep(timeout);
    receiver_handle.join().unwrap();

    // Convert results to the return format
    let results_map = results.lock().unwrap();
    targets
        .iter()
        .map(|ip| {
            let mut open_ports = results_map.get(ip).cloned().unwrap_or_default();
            open_ports.sort();
            open_ports.dedup();
            ScanResult {
                ip: *ip,
                open_ports,
            }
        })
        .collect()
}

fn send_tcp_packet(tx: &mut TransportSender, tcp_header: MutableTcpPacket<'_>, target: &IpAddr) {
    match tx.send_to(&tcp_header, *target) {
        Ok(_) => {}
        Err(e) => {
            if let Some(code) = e.raw_os_error() {
                if code == 105 {
                    thread::sleep(Duration::from_millis(500));
                    send_tcp_packet(tx, tcp_header, target);
                }
            } else {
                eprintln!("Failed to send packet: {}", e);
            }
        }
    }
}
