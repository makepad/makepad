#![no_std]
#![no_main]

use makepad_easy_esp::*;

const FRONT_CENTER: f32 = 1000.0;
const FRONT_RANGE: f32 = 550.0;
const REAR_CENTER: f32 = 1300.0;
const REAR_RANGE: f32 = 600.0;
const THROTTLE_CENTER: f32 = 1250.0;
const THROTTLE_RANGE: f32 = 600.0;
const IMU_TARGET_IP: IpAddress = IpAddress::v4(172,20,10,4);

fn main_wifi(clocks:&Clocks, pwm:Pwm, io:Io, periph:Periph, mut socket:UdpSocket){
    let mut i2c = I2C::new_with_timeout(
        periph.I2C0,
        io.pins.gpio6,
        io.pins.gpio5,
        50u32.kHz(),
        &clocks,
        Some(20),
    );
    let mut delay = Delay::new(&clocks);
    let timer = pwm.timer_low_speed(PwmTimer::Timer0, PwmDuty::Duty14Bit, 50u32.Hz());

    let throttle = pwm.channel_low_speed(&timer, PwmChannel::Channel0, io.pins.gpio2.into_push_pull_output(), 0);
    let front_wheel = pwm.channel_low_speed(&timer, PwmChannel::Channel1, io.pins.gpio3.into_push_pull_output(), 0);
    let rear_wheel = pwm.channel_low_speed(&timer, PwmChannel::Channel2, io.pins.gpio4.into_push_pull_output(), 0);
    
    init_bno055(&mut i2c, &mut delay);
    
        
    socket.bind(44441).unwrap();
    let mut wrap_counter = 0;
    loop {
        let mut buffer = [0u8; 3];
        while let Ok(_len) = socket.receive(&mut buffer) {
            let front_duty = (FRONT_CENTER - FRONT_RANGE * 0.5 + (FRONT_RANGE * buffer[0] as f32 / 255.0) ) as u32;
            front_wheel.set_duty_hw(front_duty);
            let rear_duty = (REAR_CENTER - REAR_RANGE * 0.5 + (REAR_RANGE * buffer[1] as f32 / 255.0)) as u32;
            rear_wheel.set_duty_hw(rear_duty);
            let throttle_duty = (THROTTLE_CENTER - THROTTLE_RANGE * 0.5 + (THROTTLE_RANGE * buffer[2] as f32 / 255.0)) as u32;
            println!("{}", throttle_duty);
            throttle.set_duty_hw(throttle_duty); 
            /*
            rear_wheel.set_duty_hw((FRONT_CENTER - FRONT_RANGE * 0.5 + (FRONT_RANGE * buffer[1] as f32 / 255.0)) as u32);
            throttle.set_duty_hw((THROTTLE_CENTER - THROTTLE_RANGE * 0.5 + (THROTTLE_RANGE * buffer[2] as f32 / 255.0)) as u32);*/
        }
        
        if !check_bno055(&mut i2c){
            println!("BNO055 not working");
        }
        else{
            let data = get_bno055_data(&mut i2c, wrap_counter);
            // alright lets send out a udp packet with the data
            let _ = socket.send(IMU_TARGET_IP, 44442, &data);
            wrap_counter = wrap_counter.wrapping_add(1);
        }

        let wait_end = current_millis() + 10;
        while current_millis() < wait_end {
            socket.work();
        }
    }
}

#[entry]
fn main() -> ! {
    // System inputs/clocks/etc
    wifi_and_udp_socket(
        WifiConfig::DynamicIp{
            ssid: "x",
            password: "x",
            //ip: IpAddress::v4(172,20,10,250),
            //gateway: IpAddress::v4(172,20,10,1)
        },
        main_wifi
    );
    loop{}
}

const BNO055_ADDRESS_A:u8 = 0x28;
const BNO055_ID:u8 = 0xA0;
const BNO055_CHIP_ID_ADDR:u8 = 0x00;
//const BNO055_OPERATION_MODE_NDOF:u8 = 0x0C;
const BNO055_OPERATION_MODE_IMUPLUS:u8 = 0x08;
const BNO055_OPR_MODE_ADDR:u8 = 0x3D;
const BNO055_OPERATION_MODE_CONFIG:u8 = 0x0;
const BNO055_SYS_TRIGGER_ADDR:u8 = 0x3f;
const BNO055_PAGE_ID_ADDR:u8 = 0x07;
const BNO055_POWER_MODE_NORMAL:u8 = 0x00;
const BNO055_PWR_MODE_ADDR:u8 = 0x3E;
//const BNO055_SYS_STAT_ADDR:u8 = 0x39;

const BNO055_GYRO_DATA_X_LSB_ADDR:u8 = 0x14;
const BNO055_LINEAR_ACCEL_DATA_X_LSB_ADDR:u8 = 0x28;
const BNO055_ACCEL_DATA_X_LSB_ADDR:u8 = 0x08;
const BNO055_GRAVITY_DATA_X_LSB_ADDR:u8 = 0x2E;

fn get_bno055_data(i2c:&mut I2c0, order_counter:u8)->[u8;25]{
    let mut out = [0u8; 25];
    let mut c = 0;
    out[c] = order_counter; c+=1;
    let id = BNO055_ADDRESS_A;
    i2c.write_read_addr_range(id, BNO055_GYRO_DATA_X_LSB_ADDR, &mut out[c..], 6).ok(); c+=6;
    i2c.write_read_addr_range(id, BNO055_LINEAR_ACCEL_DATA_X_LSB_ADDR, &mut out[c..], 6).ok(); c+=6;
    i2c.write_read_addr_range(id, BNO055_ACCEL_DATA_X_LSB_ADDR, &mut out[c..], 6).ok(); c+=6;
    i2c.write_read_addr_range(id, BNO055_GRAVITY_DATA_X_LSB_ADDR, &mut out[c..], 6).ok(); //c+=6;
    out
}

fn check_bno055(i2c:&mut I2c0)->bool{
    let mut data = [0u8; 1];
    i2c.write_read(BNO055_ADDRESS_A, &[BNO055_CHIP_ID_ADDR], &mut data).ok();
    data[0] == BNO055_ID
}

fn init_bno055(i2c:&mut I2c0, delay:&mut Delay){
    i2c.write(BNO055_ADDRESS_A, &[BNO055_OPR_MODE_ADDR, BNO055_OPERATION_MODE_CONFIG]).ok();
    i2c.write(BNO055_ADDRESS_A, &[BNO055_SYS_TRIGGER_ADDR, 0x20]).ok();

    loop{
        let mut data = [0u8; 1];
        i2c.write_read(BNO055_ADDRESS_A, &[BNO055_CHIP_ID_ADDR], &mut data).ok();
        if data[0] == BNO055_ID{
            break
        }
        delay.delay_ms(10u32);
    }
    delay.delay_ms(50u32);

    i2c.write(BNO055_ADDRESS_A, &[BNO055_PWR_MODE_ADDR, BNO055_POWER_MODE_NORMAL]).ok();
    delay.delay_ms(10u32);

    i2c.write(BNO055_ADDRESS_A, &[BNO055_PAGE_ID_ADDR, 0]).ok();
    i2c.write(BNO055_ADDRESS_A, &[BNO055_SYS_TRIGGER_ADDR, 0]).ok();
    delay.delay_ms(10u32);
    i2c.write(BNO055_ADDRESS_A, &[BNO055_OPR_MODE_ADDR, BNO055_OPERATION_MODE_IMUPLUS]).ok();
    delay.delay_ms(20u32);
}