//! Wi-Fi, SNTP and HTTPS for the networked examples.
//!
//! The credentials come from `secret.rs` at the repo root (git-ignored; copy `secret.rs.example`).
//!
//! TLS is `reqwless` over `embedded-tls` with no certificate check, like the sketches'
//! `client.setInsecure()`. That means anyone on the path can read what is sent, including the NewsAPI key
//! in the `news` request. Do not reuse that key anywhere else.

use alloc::{boxed::Box, vec};
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_net::{
    Config as NetConfig, Runner, Stack, StackResources,
    dns::{DnsQueryType, DnsSocket},
    tcp::client::{TcpClient, TcpClientState},
    udp,
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi::{Config, ControllerConfig, Interface, WifiController, sta::StationConfig};
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    request::{Method, RequestBuilder},
};
use sntpc::{NtpContext, get_time};
use sntpc_net_embassy::UdpSocketWrapper;
use sntpc_time_embassy::EmbassyTimestampGenerator;

/// A connected station. Keep it alive: dropping the controller shuts Wi-Fi down.
pub struct Wifi {
    pub stack: Stack<'static>,
    _controller: WifiController<'static>,
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

/// Attempts at joining the network, and at a request in [`https_get_retry`].
pub const TRIES: u32 = 3;

/// Joins the network (up to [`TRIES`] attempts of 20 s) and waits for DHCP. `None` when it does not work.
pub async fn connect(
    spawner: &Spawner,
    wifi: WIFI<'static>,
    ssid: &str,
    password: &str,
) -> Option<Wifi> {
    let station = StationConfig::default()
        .with_ssid(ssid)
        .with_password(password.into());
    let (mut controller, interfaces) = esp_radio::wifi::new(
        wifi,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    )
    .ok()?;
    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let resources = Box::leak(Box::new(StackResources::<6>::new()));
    let (stack, runner) = embassy_net::new(
        interfaces.station,
        NetConfig::dhcpv4(Default::default()),
        resources,
        seed,
    );
    spawner.spawn(net_task(runner).ok()?);

    let mut joined = false;
    for attempt in 1..=TRIES {
        if let Ok(Ok(_)) = with_timeout(Duration::from_secs(20), controller.connect_async()).await {
            joined = true;
            break;
        }
        error!("wifi join attempt {} failed", attempt);
        Timer::after(Duration::from_secs(2)).await;
    }
    if !joined {
        return None;
    }
    with_timeout(Duration::from_secs(30), stack.wait_config_up())
        .await
        .ok()?;
    if let Some(config) = stack.config_v4() {
        info!("got IP {}", config.address.address());
    }
    Some(Wifi {
        stack,
        _controller: controller,
    })
}

/// Unix seconds (UTC) from `pool.ntp.org`, or `None`.
pub async fn unix_time(stack: Stack<'static>) -> Option<i64> {
    let addrs = with_timeout(
        Duration::from_secs(10),
        stack.dns_query("pool.ntp.org", DnsQueryType::A),
    )
    .await
    .ok()?
    .ok()?;
    let Some(&embassy_net::IpAddress::Ipv4(server)) = addrs.first() else {
        return None;
    };
    let mut rx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut tx_meta = [udp::PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 256];
    let mut tx_buf = [0u8; 256];
    let mut socket =
        udp::UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(123).ok()?;
    let socket = UdpSocketWrapper::from(socket);
    let context = NtpContext::new(EmbassyTimestampGenerator::default());
    let addr = core::net::SocketAddr::new(core::net::IpAddr::V4(server), 123);
    let result = with_timeout(Duration::from_secs(10), get_time(addr, &socket, context))
        .await
        .ok()?
        .ok()?;
    i64::try_from(result.seconds).ok()
}

/// GET `url` over HTTPS into `body` and returns the number of bytes of the response body. Fails
/// (with a short reason for the log) on a connection, TLS or HTTP error, a status other than 200,
/// or a body that does not fit.
pub async fn https_get(
    stack: Stack<'static>,
    url: &str,
    user_agent: &str,
    body: &mut [u8],
) -> Result<usize, &'static str> {
    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let mut tls_read = vec![0u8; 16_640];
    let mut tls_write = vec![0u8; 4_096];
    let state = TcpClientState::<1, 1_024, 1_024>::new();
    let tcp = TcpClient::new(stack, &state);
    let dns = DnsSocket::new(stack);
    let tls = TlsConfig::new(seed, &mut tls_read, &mut tls_write, TlsVerify::None);
    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

    let request = client
        .request(Method::GET, url)
        .await
        .map_err(|_| "request failed")?;
    let headers = [("User-Agent", user_agent)];
    let mut request = request.headers(&headers);
    let response = request.send(body).await.map_err(|_| "send failed")?;
    if response.status.0 != 200 {
        error!("HTTP {}", response.status.0);
        return Err("HTTP status is not 200");
    }
    let bytes = response
        .body()
        .read_to_end()
        .await
        .map_err(|_| "body failed")?;
    info!("body {} bytes", bytes.len());
    Ok(bytes.len())
}

/// [`https_get`] up to [`TRIES`] times, 3 s apart, for a flaky link or a server that answers 5xx now and then.
pub async fn https_get_retry(
    stack: Stack<'static>,
    url: &str,
    user_agent: &str,
    body: &mut [u8],
) -> Result<usize, &'static str> {
    let mut result = Err("no attempt");
    for attempt in 1..=TRIES {
        result = https_get(stack, url, user_agent, body).await;
        match result {
            Ok(_) => break,
            Err(reason) => error!("attempt {} failed: {}", attempt, reason),
        }
        Timer::after(Duration::from_secs(3)).await;
    }
    result
}
