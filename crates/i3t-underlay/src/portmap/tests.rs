//! 端口映射对着假网关的行为测试。假网关都在回环上，说的是线上真正的字节：
//! NAT-PMP / PCP 的那个是一个 UDP socket，UPnP 的那个是一个 SSDP 应答器加一个最简单的 HTTP 服务。

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

use super::*;

const PORT: u16 = 20330;
const EXTERNAL_IP: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// 等一件"应该很快发生"的事最多等多久。
const SOON: Duration = Duration::from_secs(5);

/// 测试里用的时长：一次完整的探测在一秒之内结束。
fn fast() -> Timing {
    Timing {
        udp_first_wait: Duration::from_millis(40),
        udp_attempts: 3,
        ssdp_wait: Duration::from_millis(250),
        ssdp_searches: 2,
        http_wait: Duration::from_secs(2),
        retry: Duration::from_secs(600),
        min_renew: Duration::from_millis(100),
        release_wait: Duration::from_secs(1),
    }
}

/// 假的 NAT-PMP / PCP 网关收到过的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
enum UdpRequest {
    /// 协议号是线上的值：17 是 UDP，6 是 TCP。
    Pcp {
        protocol: u8,
        client: [u8; 16],
        nonce: [u8; 12],
        internal_port: u16,
        lifetime_secs: u32,
    },
    NatPmpAddress,
    /// 操作码是线上的值：1 是 UDP，2 是 TCP。
    NatPmpMap {
        opcode: u8,
        internal_port: u16,
        lifetime_secs: u32,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy)]
struct UdpBehaviour {
    pcp: bool,
    nat_pmp: bool,
    /// 网关肯给的最长有效期。
    lifetime_secs: u32,
    external_ip: Ipv4Addr,
    /// 有值时不管客户端希望哪个外部端口，都给这一个。
    external_port: Option<u16>,
    /// 不是 0 时 PCP 的请求一律用这个结果码拒绝。
    pcp_refusal: u8,
}

impl UdpBehaviour {
    const SILENT: Self =
        Self { pcp: false, nat_pmp: false, lifetime_secs: 3600, external_ip: EXTERNAL_IP, external_port: None, pcp_refusal: 0 };
    const NAT_PMP: Self = Self { nat_pmp: true, ..Self::SILENT };
    const PCP: Self = Self { pcp: true, ..Self::SILENT };
    const BOTH: Self = Self { pcp: true, nat_pmp: true, ..Self::SILENT };
}

/// 假的 NAT-PMP / PCP 网关。
struct FakeUdpGateway {
    addr: SocketAddr,
    behaviour: Arc<Mutex<UdpBehaviour>>,
    seen: Arc<Mutex<Vec<UdpRequest>>>,
    task: JoinHandle<()>,
}

impl Drop for FakeUdpGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeUdpGateway {
    async fn start(behaviour: UdpBehaviour) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = socket.local_addr().unwrap();
        let behaviour = Arc::new(Mutex::new(behaviour));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(Self::serve(socket, Arc::clone(&behaviour), Arc::clone(&seen)));
        Self { addr, behaviour, seen, task }
    }

    fn set(&self, behaviour: UdpBehaviour) {
        *self.behaviour.lock().unwrap() = behaviour;
    }

    fn seen(&self) -> Vec<UdpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self, wanted: impl Fn(&UdpRequest) -> bool) -> usize {
        self.seen().iter().filter(|request| wanted(request)).count()
    }

    async fn serve(socket: UdpSocket, behaviour: Arc<Mutex<UdpBehaviour>>, seen: Arc<Mutex<Vec<UdpRequest>>>) {
        let mut buf = [0u8; 1500];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buf).await else { return };
            let behaviour = *behaviour.lock().unwrap();
            let (request, reply) = Self::answer(&buf[..len], &behaviour);
            seen.lock().unwrap().push(request);
            if let Some(reply) = reply {
                socket.send_to(&reply, from).await.unwrap();
            }
        }
    }

    /// 按线上的字节认出请求、拼出回答。这里不用被测的编码器，偏移量都是照着 RFC 写的。
    fn answer(datagram: &[u8], behaviour: &UdpBehaviour) -> (UdpRequest, Option<Vec<u8>>) {
        let epoch = 9u32.to_be_bytes();
        let grant = |wanted: u32| wanted.min(behaviour.lifetime_secs).to_be_bytes();
        match *datagram {
            [0, 0] => {
                let reply = [&[0, 128, 0, 0][..], &epoch, &behaviour.external_ip.octets()].concat();
                (UdpRequest::NatPmpAddress, behaviour.nat_pmp.then_some(reply))
            }
            [0, opcode @ (1 | 2), 0, 0, i0, i1, e0, e1, l0, l1, l2, l3] => {
                let internal_port = u16::from_be_bytes([i0, i1]);
                let lifetime_secs = u32::from_be_bytes([l0, l1, l2, l3]);
                let wanted = if [e0, e1] == [0, 0] && lifetime_secs > 0 { internal_port } else { u16::from_be_bytes([e0, e1]) };
                let external = behaviour.external_port.filter(|_| lifetime_secs > 0).unwrap_or(wanted);
                let reply = [&[0, 128 + opcode, 0, 0][..], &epoch, &[i0, i1], &external.to_be_bytes(), &grant(lifetime_secs)].concat();
                (UdpRequest::NatPmpMap { opcode, internal_port, lifetime_secs }, behaviour.nat_pmp.then_some(reply))
            }
            [2, 1, ..] if datagram.len() == 60 => {
                let lifetime_secs = u32::from_be_bytes(datagram[4..8].try_into().unwrap());
                let internal_port = u16::from_be_bytes([datagram[40], datagram[41]]);
                let request = UdpRequest::Pcp {
                    protocol: datagram[36],
                    client: datagram[8..24].try_into().unwrap(),
                    nonce: datagram[24..36].try_into().unwrap(),
                    internal_port,
                    lifetime_secs,
                };
                if !behaviour.pcp {
                    // 只会 NAT-PMP 的网关按 NAT-PMP 的格式回"不支持的版本"。
                    let unsupported = [&[0, 128, 0, 1][..], &epoch].concat();
                    return (request, behaviour.nat_pmp.then_some(unsupported));
                }
                let wanted = u16::from_be_bytes([datagram[42], datagram[43]]);
                let external = behaviour.external_port.filter(|_| lifetime_secs > 0).unwrap_or(wanted);
                let mut reply = datagram.to_vec();
                reply[1] = 0x81;
                reply[2] = 0;
                reply[3] = behaviour.pcp_refusal;
                reply[4..8].copy_from_slice(&grant(lifetime_secs));
                reply[8..12].copy_from_slice(&epoch);
                reply[12..24].fill(0);
                reply[42..44].copy_from_slice(&external.to_be_bytes());
                reply[44..].copy_from_slice(&behaviour.external_ip.to_ipv6_mapped().octets());
                (request, Some(reply))
            }
            _ => (UdpRequest::Unknown, None),
        }
    }
}

/// 假的 UPnP 网关收到过的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
enum IgdRequest {
    Search,
    ExternalIp,
    Add { protocol: String, external_port: u16, internal: String, lease_secs: u32 },
    Delete { protocol: String, external_port: u16 },
}

#[derive(Debug, Clone, Copy, Default)]
struct IgdBehaviour {
    /// 不回答 SSDP 搜索。
    silent: bool,
    /// 只接受有效期是 0 的映射（错误码 725）。
    only_permanent: bool,
    /// 这个外部端口已经被别的机器占了（错误码 718）。
    taken_port: Option<u16>,
}

/// 假的 UPnP 网关。
struct FakeIgd {
    ssdp: SocketAddr,
    seen: Arc<Mutex<Vec<IgdRequest>>>,
    tasks: [JoinHandle<()>; 2],
}

impl Drop for FakeIgd {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

const SERVICE: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";

const DEVICE_DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<device>
 <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
 <friendlyName>fake</friendlyName>
 <deviceList><device>
  <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
  <deviceList><device>
   <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
   <serviceList><service>
    <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
    <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
    <SCPDURL>/scpd.xml</SCPDURL>
    <controlURL>/control</controlURL>
    <eventSubURL>/events</eventSubURL>
   </service></serviceList>
  </device></deviceList>
 </device></deviceList>
</device>
</root>"#;

fn scpd() -> String {
    let action = |name: &str, inputs: &[&str]| {
        let arguments: String =
            inputs.iter().map(|input| format!("<argument><name>{input}</name><direction>in</direction></argument>")).collect();
        format!("<action><name>{name}</name><argumentList>{arguments}</argumentList></action>")
    };
    let add = [
        "NewRemoteHost",
        "NewExternalPort",
        "NewProtocol",
        "NewInternalPort",
        "NewInternalClient",
        "NewEnabled",
        "NewPortMappingDescription",
        "NewLeaseDuration",
    ];
    format!(
        r#"<?xml version="1.0"?><scpd xmlns="urn:schemas-upnp-org:service-1-0"><actionList>{}{}{}</actionList></scpd>"#,
        action("AddPortMapping", &add),
        action("DeletePortMapping", &["NewRemoteHost", "NewExternalPort", "NewProtocol"]),
        "<action><name>GetExternalIPAddress</name><argumentList><argument><name>NewExternalIPAddress</name>\
         <direction>out</direction></argument></argumentList></action>",
    )
}

fn soap_ok(action: &str, contents: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action}Response xmlns:u="{SERVICE}">{contents}</u:{action}Response></s:Body></s:Envelope>"#
    )
}

fn soap_error(code: u16, description: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{description}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#
    )
}

/// SOAP 请求里某个参数的值。
fn argument<'a>(body: &'a str, name: &str) -> &'a str {
    let (_, rest) = body.split_once(&format!("<{name}>")).unwrap_or_else(|| panic!("请求里没有 {name}：{body}"));
    rest.split_once(&format!("</{name}>")).unwrap().0
}

impl FakeIgd {
    async fn start(behaviour: IgdBehaviour) -> Self {
        let ssdp_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let http = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let ssdp = ssdp_socket.local_addr().unwrap();
        let location = format!("http://{}/rootDesc.xml", http.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tasks = [
            tokio::spawn(Self::answer_searches(ssdp_socket, location, behaviour, Arc::clone(&seen))),
            tokio::spawn(Self::serve_http(http, behaviour, Arc::clone(&seen))),
        ];
        Self { ssdp, seen, tasks }
    }

    fn seen(&self) -> Vec<IgdRequest> {
        self.seen.lock().unwrap().clone()
    }

    async fn answer_searches(socket: UdpSocket, location: String, behaviour: IgdBehaviour, seen: Arc<Mutex<Vec<IgdRequest>>>) {
        let mut buf = [0u8; 1500];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buf).await else { return };
            if !buf[..len].starts_with(b"M-SEARCH * HTTP/1.1\r\n") {
                continue;
            }
            seen.lock().unwrap().push(IgdRequest::Search);
            if behaviour.silent {
                continue;
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                 USN: uuid:fake::urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nEXT:\r\nSERVER: fake UPnP/1.0\r\n\
                 LOCATION: {location}\r\n\r\n"
            );
            socket.send_to(reply.as_bytes(), from).await.unwrap();
        }
    }

    async fn serve_http(listener: TcpListener, behaviour: IgdBehaviour, seen: Arc<Mutex<Vec<IgdRequest>>>) {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            tokio::spawn(Self::serve_connection(stream, behaviour, Arc::clone(&seen)));
        }
    }

    /// 一条连接上只服务一个请求，答完就关。
    async fn serve_connection(mut stream: TcpStream, behaviour: IgdBehaviour, seen: Arc<Mutex<Vec<IgdRequest>>>) {
        let Some((head, body)) = read_request(&mut stream).await else { return };
        let request_line = head.lines().next().unwrap_or_default().to_owned();
        let (status, reply) = if request_line.starts_with("GET /rootDesc.xml ") {
            ("200 OK", DEVICE_DESCRIPTION.to_owned())
        } else if request_line.starts_with("GET /scpd.xml ") {
            ("200 OK", scpd())
        } else if request_line.starts_with("POST /control ") {
            let action = head
                .lines()
                .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("soapaction")))
                .map(|(_, value)| value.trim().trim_matches('"').rsplit('#').next().unwrap_or_default().to_owned())
                .unwrap_or_default();
            match Self::act(&action, &body, behaviour, &seen) {
                Ok(reply) => ("200 OK", reply),
                Err(fault) => ("500 Internal Server Error", fault),
            }
        } else {
            ("404 Not Found", String::new())
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        );
        // 客户端等不及先断开了的话写会失败，那就算了。
        let _written = stream.write_all([head.as_bytes(), reply.as_bytes()].concat().as_slice()).await;
        let _closed = stream.shutdown().await;
    }

    fn act(action: &str, body: &str, behaviour: IgdBehaviour, seen: &Mutex<Vec<IgdRequest>>) -> Result<String, String> {
        let mut seen = seen.lock().unwrap();
        match action {
            "GetExternalIPAddress" => {
                seen.push(IgdRequest::ExternalIp);
                Ok(soap_ok(action, &format!("<NewExternalIPAddress>{EXTERNAL_IP}</NewExternalIPAddress>")))
            }
            "AddPortMapping" => {
                let external_port: u16 = argument(body, "NewExternalPort").parse().unwrap();
                let lease_secs: u32 = argument(body, "NewLeaseDuration").parse().unwrap();
                let internal = format!("{}:{}", argument(body, "NewInternalClient"), argument(body, "NewInternalPort"));
                seen.push(IgdRequest::Add { protocol: argument(body, "NewProtocol").to_owned(), external_port, internal, lease_secs });
                if behaviour.only_permanent && lease_secs != 0 {
                    return Err(soap_error(725, "OnlyPermanentLeasesSupported"));
                }
                if behaviour.taken_port == Some(external_port) {
                    return Err(soap_error(718, "ConflictInMappingEntry"));
                }
                Ok(soap_ok(action, ""))
            }
            "DeletePortMapping" => {
                let external_port = argument(body, "NewExternalPort").parse().unwrap();
                seen.push(IgdRequest::Delete { protocol: argument(body, "NewProtocol").to_owned(), external_port });
                Ok(soap_ok(action, ""))
            }
            other => Err(soap_error(401, &format!("Invalid Action {other}"))),
        }
    }
}

/// 读一个 HTTP 请求，返回头和正文。
async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match stream.read(&mut chunk).await {
            Ok(len) if len > 0 => received.extend_from_slice(&chunk[..len]),
            Ok(_) | Err(_) => return None,
        }
    };
    let head = String::from_utf8_lossy(&received[..head_end]).into_owned();
    let content_length: usize = head
        .lines()
        .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("content-length")))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    let body_start = head_end + 4;
    while received.len() < body_start + content_length {
        match stream.read(&mut chunk).await {
            Ok(len) if len > 0 => received.extend_from_slice(&chunk[..len]),
            Ok(_) | Err(_) => return None,
        }
    }
    Some((head, String::from_utf8_lossy(&received[body_start..body_start + content_length]).into_owned()))
}

fn mapper(natpmp_pcp: Option<SocketAddr>, ssdp: Option<SocketAddr>) -> PortMapper {
    mapper_with(natpmp_pcp, ssdp, fast())
}

fn mapper_with(natpmp_pcp: Option<SocketAddr>, ssdp: Option<SocketAddr>, timing: Timing) -> PortMapper {
    let gateway = GatewaySource::Fixed { local: LOCALHOST, natpmp_pcp, ssdp };
    PortMapper::spawn_with(PortMapConfig { port: PORT, gateway }, timing)
}

/// 等到公布的映射满足 `wanted` 为止。
async fn wait_mapped(mapper: &PortMapper, what: &str, wanted: impl Fn(&Mapped) -> bool) -> Mapped {
    let mut updates = mapper.subscribe();
    let found = tokio::time::timeout(SOON, updates.wait_for(|mapped| wanted(mapped))).await;
    *found.unwrap_or_else(|_| panic!("映射一直没有变成{what}，现在是 {:?}", mapper.mapped())).unwrap()
}

/// 等到 `done` 为真为止。
async fn wait_until(what: &str, done: impl Fn() -> bool) {
    let waiting = async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(SOON, waiting).await.unwrap_or_else(|_| panic!("一直没有等到：{what}"));
}

fn external(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(EXTERNAL_IP), port)
}

fn is_pcp(request: &UdpRequest) -> bool {
    matches!(request, UdpRequest::Pcp { .. })
}

fn is_nat_pmp(request: &UdpRequest) -> bool {
    matches!(request, UdpRequest::NatPmpAddress | UdpRequest::NatPmpMap { .. })
}

#[tokio::test]
async fn a_nat_pmp_gateway_yields_its_public_address_and_the_port_it_chose() {
    let gateway = FakeUdpGateway::start(UdpBehaviour { external_port: Some(40000), ..UdpBehaviour::NAT_PMP }).await;
    let mapper = mapper(Some(gateway.addr), None);
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped, Mapped { udp: Some(external(40000)), tcp: None }, "网关给的外部端口和要的不一样也接受");

    let seen = gateway.seen();
    assert!(is_pcp(&seen[0]), "先试 PCP：{seen:?}");
    assert_eq!(gateway.count(is_pcp), 1, "网关说了不支持这个版本，就不再重发 PCP");
    assert_eq!(seen[1..], [UdpRequest::NatPmpAddress, UdpRequest::NatPmpMap { opcode: 1, internal_port: PORT, lifetime_secs: 7200 }]);
}

#[tokio::test]
async fn a_pcp_gateway_is_preferred_and_its_map_response_carries_the_external_address() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::BOTH).await;
    let mapper = mapper(Some(gateway.addr), None);
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped, Mapped { udp: Some(external(PORT)), tcp: None });

    let seen = gateway.seen();
    let [UdpRequest::Pcp { protocol: 17, client, internal_port: PORT, lifetime_secs: 7200, .. }] = seen[..] else {
        panic!("应该只有一个 PCP 的 UDP 映射请求，NAT-PMP 一个都不发：{seen:?}")
    };
    assert_eq!(Ipv6Addr::from(client), Ipv4Addr::LOCALHOST.to_ipv6_mapped(), "客户端地址是发请求的那个本机地址");
}

#[tokio::test]
async fn a_upnp_gateway_is_used_when_neither_udp_protocol_gets_an_answer() {
    let silent = FakeUdpGateway::start(UdpBehaviour::SILENT).await;
    let igd = FakeIgd::start(IgdBehaviour::default()).await;
    let mapper = mapper(Some(silent.addr), Some(igd.ssdp));
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped, Mapped { udp: Some(external(PORT)), tcp: None });

    let adds: Vec<IgdRequest> = igd.seen().into_iter().filter(|request| matches!(request, IgdRequest::Add { .. })).collect();
    let expected =
        IgdRequest::Add { protocol: "UDP".to_owned(), external_port: PORT, internal: format!("127.0.0.1:{PORT}"), lease_secs: 3600 };
    assert_eq!(adds, [expected]);
    assert_eq!((silent.count(is_pcp), silent.count(is_nat_pmp)), (3, 3), "前两种协议各试了三次才轮到它");
}

#[tokio::test]
async fn a_upnp_gateway_that_only_takes_permanent_leases_is_asked_again_with_lease_zero() {
    let igd = FakeIgd::start(IgdBehaviour { only_permanent: true, ..IgdBehaviour::default() }).await;
    let mapper = mapper(None, Some(igd.ssdp));
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped.udp, Some(external(PORT)));
    let leases: Vec<u32> = igd
        .seen()
        .iter()
        .filter_map(|request| if let IgdRequest::Add { lease_secs, .. } = request { Some(*lease_secs) } else { None })
        .collect();
    assert_eq!(leases, [3600, 0]);
}

#[tokio::test]
async fn a_upnp_port_taken_by_another_machine_makes_the_gateway_pick_a_different_one() {
    let igd = FakeIgd::start(IgdBehaviour { taken_port: Some(PORT), ..IgdBehaviour::default() }).await;
    let mapper = mapper(None, Some(igd.ssdp));
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    let granted = mapped.udp.unwrap();
    assert_eq!(granted.ip(), IpAddr::V4(EXTERNAL_IP));
    assert_ne!(granted.port(), PORT, "同号的端口被占了，映射出来的是另一个");
    let Some(IgdRequest::Add { external_port, internal, .. }) = igd.seen().last().cloned() else {
        panic!("最后一个请求应该是添加映射")
    };
    assert_eq!((external_port, internal), (granted.port(), format!("127.0.0.1:{PORT}")), "内部端口还是那一个");
}

#[tokio::test]
async fn the_mapping_is_renewed_at_half_of_the_lifetime_the_gateway_granted() {
    // 网关只肯给 1 秒：每半秒就要续一次。
    let gateway = FakeUdpGateway::start(UdpBehaviour { lifetime_secs: 1, ..UdpBehaviour::PCP }).await;
    let mapper = mapper(Some(gateway.addr), None);
    let mut updates = mapper.subscribe();
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    updates.borrow_and_update();

    wait_until("续了两次", || gateway.count(is_pcp) >= 3).await;
    let seen = gateway.seen();
    let nonces: Vec<[u8; 12]> =
        seen.iter().filter_map(|request| if let UdpRequest::Pcp { nonce, .. } = request { Some(*nonce) } else { None }).collect();
    assert!(nonces.windows(2).all(|pair| pair[0] == pair[1]), "续期带的是同一个随机数");
    assert!(seen.iter().all(|request| matches!(request, UdpRequest::Pcp { lifetime_secs: 7200, .. })), "{seen:?}");
    assert_eq!(mapper.mapped().udp, Some(external(PORT)));
    assert!(!updates.has_changed().unwrap(), "续期期间公布的映射没有变过");
}

#[tokio::test]
async fn a_renewal_that_fails_clears_the_mapping_and_probes_only_once_more() {
    let gateway = FakeUdpGateway::start(UdpBehaviour { lifetime_secs: 1, ..UdpBehaviour::NAT_PMP }).await;
    let mapper = mapper(Some(gateway.addr), None);
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;

    // 网关不再回答（重启了、换了一台不支持的）。
    gateway.set(UdpBehaviour::SILENT);
    let before = gateway.seen().len();
    wait_mapped(&mapper, "没有映射", |mapped| *mapped == Mapped::default()).await;
    // 续期：问地址 3 次没有回答。重新探测：PCP 3 次、NAT-PMP 问地址 3 次。之后不再发。
    wait_until("重新探测了一轮", || gateway.seen().len() - before >= 7).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after = gateway.seen().len() - before;
    assert!((7..=9).contains(&after), "续不上之后只重新探测一轮，发了 {after} 个包");
    assert_eq!(mapper.mapped(), Mapped::default());
}

/// 启用 relay：加上 TCP 的映射；停用：删掉。`deleted` 判断假网关是不是收到了删除 TCP 映射的请求。
async fn tcp_mapping_follows_set_tcp(mapper: &PortMapper, deleted: impl Fn() -> bool) {
    wait_mapped(mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    mapper.set_tcp(true);
    let mapped = wait_mapped(mapper, "有 TCP 映射", |mapped| mapped.tcp.is_some()).await;
    assert_eq!(mapped, Mapped { udp: Some(external(PORT)), tcp: Some(external(PORT)) }, "TCP 映射的是同一个端口号");
    assert!(!deleted());

    mapper.set_tcp(false);
    let mapped = wait_mapped(mapper, "没有 TCP 映射", |mapped| mapped.tcp.is_none()).await;
    assert_eq!(mapped.udp, Some(external(PORT)), "UDP 的映射不受影响");
    assert!(deleted(), "TCP 的映射在网关上删掉了");
}

#[tokio::test]
async fn set_tcp_adds_and_removes_the_tcp_mapping_over_pcp() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::PCP).await;
    let mapper = mapper(Some(gateway.addr), None);
    tcp_mapping_follows_set_tcp(&mapper, || {
        gateway.count(|request| matches!(request, UdpRequest::Pcp { protocol: 6, lifetime_secs: 0, .. })) == 1
    })
    .await;
    let nonce_of = |wanted: u8| {
        gateway.seen().into_iter().find_map(|request| match request {
            UdpRequest::Pcp { protocol, nonce, .. } if protocol == wanted => Some(nonce),
            _ => None,
        })
    };
    assert_ne!(nonce_of(17), nonce_of(6), "两个映射各有各的随机数");
}

#[tokio::test]
async fn set_tcp_adds_and_removes_the_tcp_mapping_over_nat_pmp() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::NAT_PMP).await;
    let mapper = mapper(Some(gateway.addr), None);
    let deleted = UdpRequest::NatPmpMap { opcode: 2, internal_port: PORT, lifetime_secs: 0 };
    tcp_mapping_follows_set_tcp(&mapper, || gateway.seen().contains(&deleted)).await;
    assert!(gateway.seen().contains(&UdpRequest::NatPmpMap { opcode: 2, internal_port: PORT, lifetime_secs: 7200 }));
}

#[tokio::test]
async fn set_tcp_adds_and_removes_the_tcp_mapping_over_upnp() {
    let igd = FakeIgd::start(IgdBehaviour::default()).await;
    let mapper = mapper(None, Some(igd.ssdp));
    let deleted = IgdRequest::Delete { protocol: "TCP".to_owned(), external_port: PORT };
    tcp_mapping_follows_set_tcp(&mapper, || igd.seen().contains(&deleted)).await;
}

#[tokio::test]
async fn tcp_wanted_before_the_gateway_is_found_is_mapped_together_with_udp() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::PCP).await;
    let mapper = mapper(Some(gateway.addr), None);
    mapper.set_tcp(true);
    let mapped = wait_mapped(&mapper, "两个映射都有", |mapped| mapped.udp.is_some() && mapped.tcp.is_some()).await;
    assert_eq!(mapped, Mapped { udp: Some(external(PORT)), tcp: Some(external(PORT)) });
}

#[tokio::test]
async fn shutdown_deletes_the_mappings_over_pcp() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::PCP).await;
    let mapper = mapper(Some(gateway.addr), None);
    mapper.set_tcp(true);
    wait_mapped(&mapper, "两个映射都有", |mapped| mapped.udp.is_some() && mapped.tcp.is_some()).await;
    let updates = mapper.subscribe();
    mapper.shutdown().await;

    let deletions: Vec<u8> = gateway
        .seen()
        .iter()
        .filter_map(|request| if let UdpRequest::Pcp { protocol, lifetime_secs: 0, .. } = request { Some(*protocol) } else { None })
        .collect();
    assert_eq!(deletions.len(), 2, "两个映射都删了：{:?}", gateway.seen());
    assert!(deletions.contains(&17) && deletions.contains(&6));
    assert_eq!(*updates.borrow(), Mapped::default(), "停下之后不再公布映射");
}

#[tokio::test]
async fn shutdown_deletes_the_mappings_over_nat_pmp() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::NAT_PMP).await;
    let mapper = mapper(Some(gateway.addr), None);
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    mapper.shutdown().await;
    assert!(gateway.seen().contains(&UdpRequest::NatPmpMap { opcode: 1, internal_port: PORT, lifetime_secs: 0 }));
    assert!(!gateway.seen().contains(&UdpRequest::NatPmpMap { opcode: 2, internal_port: PORT, lifetime_secs: 0 }), "没有做过的映射不去删");
}

#[tokio::test]
async fn shutdown_deletes_the_mappings_over_upnp() {
    let igd = FakeIgd::start(IgdBehaviour::default()).await;
    let mapper = mapper(None, Some(igd.ssdp));
    mapper.set_tcp(true);
    wait_mapped(&mapper, "两个映射都有", |mapped| mapped.udp.is_some() && mapped.tcp.is_some()).await;
    mapper.shutdown().await;
    for protocol in ["UDP", "TCP"] {
        assert!(igd.seen().contains(&IgdRequest::Delete { protocol: protocol.to_owned(), external_port: PORT }), "{protocol}");
    }
}

#[tokio::test]
async fn shutdown_gives_up_quickly_when_the_gateway_no_longer_answers() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::PCP).await;
    let timing = Timing { release_wait: Duration::from_millis(200), ..fast() };
    let mapper = mapper_with(Some(gateway.addr), None, timing);
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    gateway.set(UdpBehaviour::SILENT);
    let started = Instant::now();
    mapper.shutdown().await;
    assert!(started.elapsed() < Duration::from_millis(600), "等了 {:?}", started.elapsed());
}

#[tokio::test]
async fn dropping_the_handle_still_removes_the_mapping_in_the_background() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::NAT_PMP).await;
    let mapper = mapper(Some(gateway.addr), None);
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    drop(mapper);
    let deleted = UdpRequest::NatPmpMap { opcode: 1, internal_port: PORT, lifetime_secs: 0 };
    wait_until("后台任务删掉映射", || gateway.seen().contains(&deleted)).await;
}

#[tokio::test]
async fn a_gateway_that_supports_nothing_costs_a_bounded_number_of_packets_and_yields_no_mapping() {
    let silent = FakeUdpGateway::start(UdpBehaviour::SILENT).await;
    let igd = FakeIgd::start(IgdBehaviour { silent: true, ..IgdBehaviour::default() }).await;
    let mapper = mapper(Some(silent.addr), Some(igd.ssdp));

    // 一轮探测：PCP 和 NAT-PMP 各等 40 + 80 + 160 毫秒，SSDP 搜两次、每次 250 毫秒。
    wait_until("一轮探测发完", || silent.seen().len() >= 6 && igd.seen().len() >= 6).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mapper.mapped(), Mapped::default());
    assert_eq!((silent.count(is_pcp), silent.count(is_nat_pmp)), (3, 3), "{:?}", silent.seen());
    assert_eq!(
        silent.seen()[3..],
        [UdpRequest::NatPmpAddress, UdpRequest::NatPmpAddress, UdpRequest::NatPmpAddress],
        "没有回答就不会去要映射"
    );
    assert_eq!(igd.seen(), vec![IgdRequest::Search; 6], "两次搜索，每次三种设备类型各问一个包");

    // 之后一个包都不再发。
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!((silent.seen().len(), igd.seen().len()), (6, 6));
}

#[tokio::test]
async fn after_a_failed_probe_nothing_happens_until_the_network_changes() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::SILENT).await;
    let mapper = mapper(Some(gateway.addr), None);
    wait_until("第一轮探测结束", || gateway.seen().len() == 6).await;
    assert_eq!(mapper.mapped(), Mapped::default());

    // 换了一个支持 NAT-PMP 的网络。
    gateway.set(UdpBehaviour::NAT_PMP);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mapper.mapped(), Mapped::default(), "没有人告诉它网络变了，它不会自己去试");
    mapper.network_changed();
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped.udp, Some(external(PORT)));
}

#[tokio::test]
async fn a_network_change_that_leaves_the_gateway_as_it_was_only_renews_the_mapping() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::PCP).await;
    let mapper = mapper(Some(gateway.addr), None);
    let mut updates = mapper.subscribe();
    wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    updates.borrow_and_update();

    mapper.network_changed();
    wait_until("提前续了一次", || gateway.seen().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(gateway.seen().len(), 2, "只续一次，不重新探测");
    assert!(!updates.has_changed().unwrap(), "公布的映射没有闪断");
    assert_eq!(mapper.mapped().udp, Some(external(PORT)));
}

#[tokio::test]
async fn a_gateway_without_a_public_address_yields_no_mapping() {
    let gateway = FakeUdpGateway::start(UdpBehaviour { external_ip: Ipv4Addr::UNSPECIFIED, ..UdpBehaviour::NAT_PMP }).await;
    let mapper = mapper(Some(gateway.addr), None);
    wait_until("NAT-PMP 问过地址", || gateway.seen().contains(&UdpRequest::NatPmpAddress)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mapper.mapped(), Mapped::default());
    assert_eq!(gateway.count(|request| matches!(request, UdpRequest::NatPmpMap { .. })), 0, "网关没有公网地址，就不去要映射");
}

#[tokio::test]
async fn nothing_is_sent_when_there_is_no_ipv4_address_to_map() {
    let gateway = FakeUdpGateway::start(UdpBehaviour::BOTH).await;
    let igd = FakeIgd::start(IgdBehaviour::default()).await;
    let source = GatewaySource::Fixed { local: IpAddr::V6(Ipv6Addr::LOCALHOST), natpmp_pcp: Some(gateway.addr), ssdp: Some(igd.ssdp) };
    let mapper = PortMapper::spawn_with(PortMapConfig { port: PORT, gateway: source }, fast());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mapper.mapped(), Mapped::default());
    assert!(gateway.seen().is_empty() && igd.seen().is_empty());
    mapper.shutdown().await;
}

#[tokio::test]
async fn a_pcp_refusal_is_not_retried_and_nat_pmp_is_tried_next() {
    // 结果码 2：网关上有一个随机数不同的旧映射（上一次运行留下的），不认这个请求。
    let gateway = FakeUdpGateway::start(UdpBehaviour { pcp_refusal: 2, ..UdpBehaviour::BOTH }).await;
    let mapper = mapper(Some(gateway.addr), None);
    let mapped = wait_mapped(&mapper, "有 UDP 映射", |mapped| mapped.udp.is_some()).await;
    assert_eq!(mapped.udp, Some(external(PORT)));
    assert_eq!(gateway.count(is_pcp), 1, "网关明确拒绝了，不用重发");
    assert!(gateway.seen().contains(&UdpRequest::NatPmpMap { opcode: 1, internal_port: PORT, lifetime_secs: 7200 }));
}

#[test]
fn the_system_gateway_is_asked_on_the_standard_ports_and_only_from_behind_a_translator() {
    let route = |local: &str| gateway::DefaultRoute { gateway: Ipv4Addr::new(192, 168, 1, 1), local: local.parse().unwrap() };
    let target = Target::behind(route("192.168.1.5")).expect("私有地址要映射");
    assert_eq!(target.local, Ipv4Addr::new(192, 168, 1, 5));
    assert_eq!(target.udp, Some("192.168.1.1:5351".parse().unwrap()));
    let ssdp = target.ssdp.expect("也试 UPnP");
    assert_eq!(ssdp.to, "239.255.255.250:1900".parse().unwrap());
    assert_eq!(ssdp.only, Some("192.168.1.1".parse().unwrap()), "只认默认网关上的设备");
    assert!(ssdp.trusted.is_empty());

    assert_eq!(Target::behind(route("203.0.113.7")), None, "本机已经是公网地址：什么都不发");
}
