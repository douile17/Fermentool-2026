//! MODBUS-RTU framing helpers for the Baoding Shenchen **LabQ** peristaltic pump.
//!
//! Milestone 1 (this file): CRC-16, the `f32` word encoding used by function
//! code `10H`, and the register map — all checked byte-for-byte against the
//! vendor examples in `LabQ Series MODBUS protocol.md`.
//!
//! Milestone 3 adds the full frame builders/parsers, the async RTU client over
//! `tokio-serial`, and the `PumpTransport` trait (`docs/IMPLEMENTATION_PLAN.md` §4.3).

/// LabQ holding-register addresses (decimal).
pub mod reg {
    /// Pump head type — `u16`, function code `06H`. See Chart 1 (KT15 = 0).
    pub const PUMP_HEAD_TYPE: u16 = 1000;
    /// Tubing size — `u16`, `06H`.
    pub const TUBING_SIZE: u16 = 1001;
    /// Motor speed, 0.1..=350 rpm — `f32` (2 registers), function code `10H`.
    pub const MOTOR_SPEED: u16 = 1002;
    /// Flow rate, 0..=99999 ml/min — `f32` (2 registers), `10H`.
    pub const FLOW_RATE: u16 = 1004;
    /// Start/stop — `u16`, `06H`. `1` = start, `0` = stop.
    pub const START_STOP: u16 = 1006;
    /// Direction — `u16`, `06H`. `1` = clockwise, `0` = counter-clockwise.
    pub const DIRECTION: u16 = 1007;
    /// Full-speed run — `u16`, `06H`. `1` = start, `0` = stop.
    pub const FULL_SPEED: u16 = 1008;
    /// Back-suction angle, 0..=360° — `u16`, `06H`.
    pub const BACK_SUCTION_ANGLE: u16 = 1009;
}

/// MODBUS CRC-16 (polynomial `0xA001`, initial value `0xFFFF`).
///
/// The returned word is transmitted **low byte first, then high byte**
/// (see [`with_crc`]).
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Append the CRC to a frame body: low byte, then high byte.
pub fn with_crc(mut frame: Vec<u8>) -> Vec<u8> {
    let crc = crc16(&frame);
    frame.push((crc & 0x00FF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// Encode an `f32` the way the pump expects for function code `10H`:
/// IEEE-754, big-endian byte order (`8.9` → `41 0E 66 66`).
pub fn f32_be(value: f32) -> [u8; 4] {
    value.to_be_bytes()
}

/// Decode a big-endian IEEE-754 `f32` from a 4-byte register pair.
pub fn f32_from_be(bytes: [u8; 4]) -> f32 {
    f32::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_vendor_start_frame() {
        // "01 06 03 EE 00 01" + CRC "28 7B"  — start pump, address 1.
        let body = [0x01, 0x06, 0x03, 0xEE, 0x00, 0x01];
        let crc = crc16(&body);
        assert_eq!((crc & 0x00FF) as u8, 0x28, "CRC low byte");
        assert_eq!((crc >> 8) as u8, 0x7B, "CRC high byte");
    }

    #[test]
    fn with_crc_matches_vendor_direction_frame() {
        // "01 06 03 EF 00 01" + CRC "79 BB"  — direction clockwise.
        assert_eq!(
            with_crc(vec![0x01, 0x06, 0x03, 0xEF, 0x00, 0x01]),
            vec![0x01, 0x06, 0x03, 0xEF, 0x00, 0x01, 0x79, 0xBB]
        );
    }

    #[test]
    fn float_encoding_matches_vendor_example() {
        assert_eq!(f32_be(8.9), [0x41, 0x0E, 0x66, 0x66]);
        assert_eq!(f32_from_be([0x41, 0x0E, 0x66, 0x66]), 8.9);
    }

    #[test]
    fn speed_58_8_matches_vendor_write_frame() {
        // "01 10 03 EA 00 02 04" + f32(58.8) + CRC "58 29".
        let mut body = vec![0x01, 0x10, 0x03, 0xEA, 0x00, 0x02, 0x04];
        body.extend_from_slice(&f32_be(58.8));
        assert_eq!(
            with_crc(body),
            vec![
                0x01, 0x10, 0x03, 0xEA, 0x00, 0x02, 0x04, //
                0x42, 0x6B, 0x33, 0x33, // f32(58.8) big-endian
                0x58, 0x29, // CRC
            ]
        );
    }

    #[test]
    fn flow_50_matches_vendor_write_frame() {
        // "01 10 03 EC 00 02 04" + f32(50.0) + CRC "7D 2C".
        let mut body = vec![0x01, 0x10, 0x03, 0xEC, 0x00, 0x02, 0x04];
        body.extend_from_slice(&f32_be(50.0));
        assert_eq!(
            with_crc(body),
            vec![
                0x01, 0x10, 0x03, 0xEC, 0x00, 0x02, 0x04, //
                0x42, 0x48, 0x00, 0x00, //
                0x7D, 0x2C,
            ]
        );
    }

    #[test]
    fn register_addresses_are_the_documented_decimals() {
        assert_eq!(reg::MOTOR_SPEED, 1002);
        assert_eq!(reg::FLOW_RATE, 1004);
        assert_eq!(reg::START_STOP, 1006);
        assert_eq!(reg::DIRECTION, 1007);
        // 1002 decimal == 0x03EA, as used in the vendor frames above.
        assert_eq!(reg::MOTOR_SPEED, 0x03EA);
    }
}
