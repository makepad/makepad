#![no_std]

pub use embedded_svc;
pub use esp_wifi;
pub use hal;
pub use smoltcp;
pub use fugit;

use embedded_svc::ipv4 as ipv4_svc;
use embedded_svc::wifi as wifi_svc;
use embedded_svc::{wifi::Wifi, ipv4::Interface};
use esp_backtrace as _;

use esp_wifi::wifi::utils::create_network_interface;
use esp_wifi::wifi::{WifiStaDevice};
use esp_wifi::wifi_interface::{WifiStack,UdpSocket as UdpSocket2};
use esp_wifi::{EspWifiInitFor};
use hal::clock::{ClockControl};
use hal::gpio::{OutputPin};
use hal::peripheral::Peripheral;
use hal::{Rng, IO, ledc, ledc::LEDC};
use hal::{peripherals::Peripherals};
use smoltcp::iface::SocketStorage;
use smoltcp::socket::udp::PacketMetadata;

pub use hal::prelude::*;
pub use fugit::{RateExtU32,HertzU32};
pub use hal::clock::Clocks;
pub use esp_println::{println};
pub use hal::entry;
pub use hal::i2c::I2C;
pub use esp_wifi::current_millis;
pub use smoltcp::wire::IpAddress;
pub use hal::Delay;
pub use hal::peripherals::I2C0;

pub trait I2CExt<'a>{
    fn write_read_addr_range(&mut self, base:u8, addr:u8, out:&mut [u8], steps:usize)->Result<(), hal::i2c::Error>;
}

impl<'a> I2CExt<'a> for I2C<'a, I2C0>{
    fn write_read_addr_range(&mut self, base:u8, addr:u8, out:&mut [u8], steps:usize)->Result<(), hal::i2c::Error>{
        for i in 0..steps{
            let mut data = [0u8;1];
            self.write_read(base, &[addr + i as u8], &mut data)?;
            out[i] = data[0];
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub enum WifiConfig{
    StaticIp{
        ssid:&'static str,
        password:&'static str,
        ip:IpAddress,
        gateway:IpAddress
    },
    DynamicIp{
        ssid:&'static str,
        password:&'static str,
    }
}

impl WifiConfig{
    pub fn ssid_password(&self)->(&'static str, &'static str){
        match self{
            WifiConfig::StaticIp{ssid, password,..}=>(ssid, password),
            WifiConfig::DynamicIp{ssid, password,..}=>(ssid, password)
        }
    }
}
pub type Pwm<'a> = LEDC<'a>;
pub type Io<'a> = IO;
pub type UdpSocket<'s, 'a> = UdpSocket2<'s, 'a, WifiStaDevice>;
pub type I2c0<'a> = I2C<'a, I2C0>;

#[allow(non_snake_case)]
pub struct Periph{
    pub I2C0:I2C0 
}

pub  fn wifi_and_udp_socket<F>(wifi_config:WifiConfig, f:F) where F:FnOnce(&Clocks, Pwm, Io, Periph, UdpSocket){

    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();
    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let timer = hal::systimer::SystemTimer::new(peripherals.SYSTIMER).alarm0;
    let mut pwm = LEDC::new(peripherals.LEDC, &clocks);
    pwm.set_global_slow_clock(ledc::LSGlobalClkSource::APBClk);

    // alright lets connect wifi
    let wifi_init = esp_wifi::initialize(
        EspWifiInitFor::Wifi,
        timer,
        Rng::new(peripherals.RNG),
        system.radio_clock_control,
        &clocks,
    ).unwrap();

    let (ssid, password) = wifi_config.ssid_password();

    let wifi = peripherals.WIFI;
    let mut socket_set_entries: [SocketStorage; 3] = Default::default();
    let (wifi_iface, wifi_device, mut wifi_controller, wifi_sockets) =
        create_network_interface(&wifi_init, wifi, WifiStaDevice, &mut socket_set_entries).unwrap();
    let mut wifi_stack = WifiStack::new(wifi_iface, wifi_device, wifi_sockets, current_millis);

    let wifi_controller_config = wifi_svc::Configuration::Client(wifi_svc::ClientConfiguration {
        ssid: ssid.try_into().unwrap(),
        password: password.try_into().unwrap(),
        ..Default::default()
    });
    let res = wifi_controller.set_configuration(&wifi_controller_config);
    println!("wifi_set_configuration returned {:?}", res);

    wifi_controller.start().unwrap();
    println!("is wifi started: {:?}", wifi_controller.is_started());

    /*
    println!("Start Wifi Scan");
    let res: Result<(heapless::Vec<wifi_svc::AccessPointInfo, 10>, usize), WifiError> = controller.scan_n();
    if let Ok((res, _count)) = res {
        for ap in res {
            println!("{:?}", ap);
        }
    }*/

    println!("{:?}", wifi_controller.get_capabilities());
    println!("wifi_connect {:?}", wifi_controller.connect());

    // wait to get connected
    println!("Wait to get connected");
    loop {
        let res = wifi_controller.is_connected();
        match res {
            Ok(connected) => {
                if connected {
                    break;
                }
            }
            Err(err) => {
                println!("Error: {:?}", err);
                loop {}
            }
        }
    }
    println!("{:?}", wifi_controller.is_connected());
    
    fn ip_address_convert(ip:IpAddress)->ipv4_svc::Ipv4Addr{
        match ip{
            IpAddress::Ipv4(addr) => ipv4_svc::Ipv4Addr::from(addr.0),
            _=>{panic!()}
        }
    }
    
    // static IP
    if let WifiConfig::StaticIp{ip, gateway, ..} = wifi_config{
        wifi_stack.set_iface_configuration(&ipv4_svc::Configuration::Client(
            ipv4_svc::ClientConfiguration::Fixed(ipv4_svc::ClientSettings {
                ip: ip_address_convert(ip),
                subnet: ipv4_svc::Subnet {
                    gateway: ip_address_convert(gateway),
                    mask: ipv4_svc::Mask(24),
                },
                dns: None,
                secondary_dns: None,
            }),
        )).unwrap();            
    }
    else{
        println!("Wait to get an ip address");
        loop {
            wifi_stack.work();
    
            if wifi_stack.is_iface_up() {
                println!("got ip {:?}", wifi_stack.get_ip_info());
                break;
            }
        }
    }
    
    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let mut rx_meta = [PacketMetadata::EMPTY;3];
    let mut tx_meta = [PacketMetadata::EMPTY;3];
    let mut socket = wifi_stack.get_udp_socket(&mut rx_meta, &mut rx_buffer, &mut tx_meta, &mut tx_buffer);
    socket.work();
    
    f(&clocks, pwm, io, Periph{
        I2C0: peripherals.I2C0
    }, socket);
}

pub type PwmTimerLowSpeed<'a> = ledc::timer::Timer<'a, ledc::LowSpeed>;
pub type PwmChannelLowSpeed<'a, O> = ledc::channel::Channel<'a, ledc::LowSpeed, O>;
pub type PwmChannel = ledc::channel::Number;
pub type PwmTimer = ledc::timer::Number;
pub type PwmDuty = ledc::timer::config::Duty;

pub trait PwmExt<'a>{
    fn timer_low_speed(&self, number: PwmTimer, duty:PwmDuty, freq:HertzU32)->PwmTimerLowSpeed;
    fn channel_low_speed<O:  OutputPin>(&self, timer:&'a PwmTimerLowSpeed, number: PwmChannel,  output_pin: impl Peripheral<P = O> + 'a, init:u32)->PwmChannelLowSpeed<O>;
}

impl<'a> PwmExt<'a> for Pwm<'a>{
    fn timer_low_speed(&self, number: PwmTimer, duty:PwmDuty, frequency:HertzU32)->PwmTimerLowSpeed{
        let mut lstimer = self.get_timer::<ledc::LowSpeed>(number);
        lstimer.configure(ledc::timer::config::Config {
            duty,
            clock_source: ledc::timer::LSClockSource::APBClk,
            frequency,
        }).unwrap();
        lstimer
    }

    fn channel_low_speed<O:  OutputPin>(&self, timer:&'a PwmTimerLowSpeed, number: PwmChannel, output_pin: impl Peripheral<P = O> + 'a, init:u32)->PwmChannelLowSpeed<O>{
        let mut chan = self.get_channel(number, output_pin);
        chan.configure(ledc::channel::config::Config {
            timer: timer,
            duty_pct: 0,
            pin_config: ledc::channel::config::PinConfig::PushPull,
        }).unwrap();
        chan.set_duty_hw(init);
        chan
    }
}