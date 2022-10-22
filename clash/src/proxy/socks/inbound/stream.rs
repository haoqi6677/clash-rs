use crate::proxy::datagram::InboundUdp;
use crate::proxy::socks::inbound::datagram::Socks5UDPCodec;
use crate::proxy::socks::inbound::{auth_methods, response_code, socks_command, SOCKS_VERSION};
use crate::session::{Network, Session, SocksAddr};
use crate::Dispatcher;
use bytes::{BufMut, BytesMut};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::{io, str};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{sleep_until, Instant};
use tokio_util::udp::UdpFramed;

pub(crate) async fn handle_tcp(
    sess: &mut Session,
    s: &mut TcpStream,
    dispatcher: Arc<Dispatcher>,
    users: &HashMap<String, String>,
) -> io::Result<()> {
    // handshake
    let mut buf = BytesMut::new();
    {
        buf.resize(2, 0);
        s.read_exact(&mut buf[..]).await?;

        if buf[0] != SOCKS_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "unsupported SOCKS version",
            ));
        }

        let n_methods = buf[1] as usize;
        if n_methods == 0 {
            return Err(io::Error::new(io::ErrorKind::Other, "malformed SOCKS data"));
        }

        buf.resize(n_methods, 0);
        s.read_exact(&mut buf[..]).await?;

        let mut response = [SOCKS_VERSION, auth_methods::NO_METHODS];
        let methods = &buf[..];
        if methods.contains(&auth_methods::USER_PASS) {
            response[1] = auth_methods::USER_PASS;
            s.write_all(&response).await?;

            buf.resize(2, 0);
            s.read_exact(&mut buf[..]).await?;
            let ulen = buf[1] as usize;
            buf.resize(ulen, 0);
            s.read_exact(&mut buf[..]).await?;
            let user = unsafe { str::from_utf8_unchecked(buf.to_owned().as_ref()).to_owned() };

            s.read_exact(&mut buf[..1]).await?;
            let plen = buf[0] as usize;
            buf.resize(plen, 0);
            s.read_exact(&mut buf[..]).await?;
            let pass = unsafe { str::from_utf8_unchecked(buf.to_owned().as_ref()).to_owned() };

            match users.get(&user) {
                Some(p) if p == &pass => {
                    response = [0x1, response_code::SUCCEEDED];
                    s.write_all(&response).await?;
                }
                _ => {
                    response = [0x1, response_code::FAILURE];
                    s.write_all(&response).await?;
                    s.shutdown().await?;
                    return Err(io::Error::new(io::ErrorKind::Other, "auth failure"));
                }
            }
        } else if methods.contains(&auth_methods::NO_AUTH) {
            response[1] = auth_methods::NO_AUTH;
            s.write_all(&response).await?;
        } else {
            response[1] = auth_methods::NO_METHODS;
            s.write_all(&response).await?;
            s.shutdown().await?;
            return Err(io::Error::new(io::ErrorKind::Other, "auth failure"));
        }
    }

    buf.resize(3, 0);
    s.read_exact(&mut buf[..]).await?;
    if buf[0] != SOCKS_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "unsupported SOCKS version",
        ));
    }

    let dst = SocksAddr::read_from(s).await?;

    match buf[1] {
        socks_command::CONNECT => {
            debug!("Got a CONNECT request from {}", s.peer_addr()?);

            buf.clear();
            buf.put_u8(SOCKS_VERSION);
            buf.put_u8(response_code::SUCCEEDED);
            buf.put_u8(0x0);
            let bnd = SocksAddr::from(s.local_addr()?);
            bnd.write_buf(&mut buf);
            s.write_all(&buf[..]).await?;
            sess.destination = dst;

            dispatcher
                .dispatch_stream(sess.to_owned(), Box::new(s) as _)
                .await;

            Ok(())
        }
        socks_command::UDP_ASSOCIATE => {
            let udp_addr = SocketAddr::new(s.local_addr()?.ip(), 0);
            let udp_inbound = UdpSocket::bind(&udp_addr).await?;

            debug!(
                "Got a UDP_ASSOCIATE request from {}, UDP assigned at {}",
                s.peer_addr()?,
                udp_inbound.local_addr()?
            );

            buf.clear();
            buf.put_u8(SOCKS_VERSION);
            buf.put_u8(response_code::SUCCEEDED);
            buf.put_u8(0x0);
            let bnd = SocksAddr::from(udp_inbound.local_addr()?);
            bnd.write_buf(&mut buf);

            let (close_handle, mut close_listener) = tokio::sync::oneshot::channel();

            let framed = UdpFramed::new(udp_inbound, Socks5UDPCodec);

            let sess = Session {
                network: Network::UDP,
                packet_mark: None,
                iface: None,
                ..Default::default()
            };

            let dispatcher_cloned = dispatcher.clone();

            tokio::spawn(async move {
                tokio::select! {
                    _ = dispatcher_cloned.dispatch_datagram(sess, Box::new(InboundUdp::new(framed))) => {
                        debug!("UDP dispatch finished, maybe with error")
                    },
                    _ = &mut close_listener => {
                        debug!("UDP association finished, dropping handle")
                    }
                }
            });

            s.write_all(&buf[..]).await?;

            buf.resize(1, 0);
            match s.read(&mut buf[..]).await {
                Ok(_) => {
                    warn!("Unexpected data from SOCKS client, dropping connection");
                }
                Err(e) => {
                    debug!(
                        "socket became error after UDP ASSOCIATE, maybe closed: {}",
                        e
                    );
                }
            }

            let _ = close_handle.send(1);

            Ok(())
        }
        _ => {
            buf.clear();
            buf.put_u8(SOCKS_VERSION);
            buf.put_u8(response_code::COMMAND_NOT_SUPPORTED);
            buf.put_u8(0x0);
            SocksAddr::any_ipv4().write_buf(&mut buf);
            s.write_all(&buf).await?;
            Err(io::Error::new(
                io::ErrorKind::Other,
                "unsupported SOCKS command",
            ))
        }
    }
}