# Lab V Series MODBUS Communication Protocol

Source: Baoding Shenchen Precision Pump Co., Ltd., 2015,
https://darwin-microfluidics.com/content/pdfs/Lab-V-Series-MODBUS-Protocol.pdf
(www.good-pump.com). Converted to Markdown from the PDF text; the content is
unchanged, apart from the pump head / tubing table (see the note under it).

Note: The hexadecimal numbers are expressed by `XXXXH` or `XXH` in the below
description.

## 1. MODBUS-RTU standard communication format

This communication uses MODBUS RTU mode, message frame as below:

| Slave address | Function code | Data area | CRC Check (Cyclic Redundancy Check) |
|---|---|---|---|
| 1 Byte | 1 Byte | 0 or up to 252 bytes | 2 Bytes (CRC low, CRC high) |

1. Slave address: Host control peristaltic pump address No. The pump address No.
   should not be same when they are in the same 485 line. The address No. range
   is 1~32, 0 means broadcast.
2. Function code: The protocol uses common function codes defined by the MODBUS
   protocol:
   - 03H: Read holding registers
   - 06H: Write single register
   - 10H: Write multiple registers
   - 02H: Read discrete inputs (read bits of data)
   - 05H: Write single bit to register
3. Data area: The detailed information command that the peristaltic pumps need
   to follow, such as start/stop, change direction, increase/decrease speed and
   so on.
4. CRC check: CRC code is 2 bytes, 16 check codes. Use CRC-16 (which used in
   American binary synchronous system). Polynomial: G(X)=X16+X15+X2+1. CRC check
   C language code: see Appendix 1.

## 2. Communication setting

1. Communication baudrate: 1200, 2400, 4800, 9600 optional.
2. Byte structure: 1 start bit + 8 data bits + 1 parity bit + 1 stop bit.
3. Bit serial sending order: the least significant bit (LSB) first ... the most
   significant bit (MSB) last: Start, 1, 2, 3, 4, 5, 6, 7, 8, Check, Stop.
4. Data transferring format:
   - Integer (2 bytes): Data: the second byte, the first byte. Send: the second
     byte, the first byte. For example: 1234H is sent 12H 34H.
   - Long integer and Float (4 bytes): Data: the fourth byte, the third byte,
     the second byte, the first byte. Send: the fourth byte, the third byte,
     the second byte, the first byte. For example: 12345678H is sent
     12H 34H 56H 78H.

## 3. MODBUS message RTU frame format

| Start | Address | Function code | Data | CRC check code | End |
|---|---|---|---|---|---|
| 3.5 characters | 8 bits | 8 bits | N x 8 bits | 16 bits | 3.5 characters |

The entire message frame must be sent in a continuous stream of characters. If
the idle space between two characters is greater than 1.5 character times, the
message frame is considered as incomplete, should be discarded by receiving
node.

## 4. Abnormal reaction

When host sending request data, slave receiving data abnormal, it should have
abnormal reaction. If the address code sent from host is wrong, there is no this
address code between slaves or the data received by slave is wrong when CRC
check, no abnormal code return, the host should have super reaction process.

Function code area: Abnormal reaction function code is normal reaction function
code + 80H. Data area: Return abnormal code, define as below.

Chart 1: Abnormal code definition

| Code | Name | Meaning |
|---|---|---|
| 01H | Illegal function code | The function code received by peristaltic pump except 03H/06H/10H. |
| 02H | Illegal data address | This abnormal code means the register address is not allowed data which received by peristaltic pump. |
| 03H | Illegal data value | Written data does not meet the operating range. |
| 06H | Slave (peristaltic pump) busy | The current state of the peristaltic pump conflict with the command received, unable to complete the command. |

Peristaltic Pump only receive MODBUS command with the Main Interface, other
interface do not receive message.

## 5. Holding register address and content

### Basic parameters setting

| Address (decimal) | Name | Range | Data type |
|---|---|---|---|
| 1000 | Pump Head | Relative data refer to the Pump Head & Tubing No. chart | unsigned short int, 2 bytes |
| 1001 | Tubing Size | Relative data refer to the Pump Head & Tubing No. chart | unsigned short int, 2 bytes |
| 1002 | Motor Speed | 0.1-600 rpm | float, 4 bytes |
| 1004 | Flow Rate | 0.1-99999 mL | float, 4 bytes |
| 1007 | Back Suction Angle | 0-360 degrees | unsigned short int, 2 bytes |
| 1008 | Start/Stop Control | 1 Start, 0 Stop | unsigned short int, 2 bytes |
| 1009 | Direction Control | 1 Clockwise, 0 Anticlockwise | unsigned short int, 2 bytes |
| 1010 | Full Speed Running | 1 Start full speed, 0 Stop full speed | unsigned short int, 2 bytes |
| 1015 | Set Flow Volume | 0-99999 mL | float, 4 bytes |
| 1018 | Working Time | 0.1-9999 s | float, 4 bytes |
| 1020 | Working Mode | 0: Transferring, 1: Fixed volume measurement, 2: Fixed time and volume | unsigned short int, 2 bytes |
| 1021 | Pause Time | 0.1-9999 s | float, 4 bytes |
| 1023 | Copy Numbers | 0-9999, 0 means infinite | unsigned short int, 2 bytes |

Notes:

- When working mode is Transferring, set up register 1015 and 1018 is invalid.
- When working mode is Fixed volume measurement, set up register 1018 invalid.
- When working mode is Fixed time and volume, set up register 1002 and 1004
  invalid.
- Please set up the register data refer to the chart, it can not receive one
  order with setting up multiple registers.

### Calibration parameters setting up

| Address (decimal) | Name | Range | Data type |
|---|---|---|---|
| 2001 | Testing time | 0.5-9999 s | unsigned short int, 2 bytes |
| 2002 | Start test | 1 Start, 0 Stop | unsigned short int, 2 bytes |
| 2003 | Actual volume | 0-9999 mL | unsigned short int, 2 bytes |
| 2005 | Restore Defaults | 1 Restore calibration | |
| 2006 | Micro Adjustment | 1 Increase, 0 Decrease | float, 4 bytes |

The PDF text extraction scrambled the data type column of this table; the
types above follow the original row order and should be checked against the PDF
before relying on them.

### Chart: pump head and tubing No.

Pump head numbers (register 1000): YZ1515x = 0, YZ2515x = 1, 2*YZ1515x = 2,
2*YZ2515x = 3, MCn(10) = 4, MCn(6) = 5, DZ25-3L = 6, SN15 = 7, SN25 = 8.

Tubing sizes (register 1001), with the tubing specification beside them:
13 (13#), 14 (14#), 19 (19#), 16 (16#), 25 (25#), 17 (17#), 18 (18#), 15 (15#),
24 (24#), 35 (35#), 36 (36#), 101 (1*1), 102 (2*1), 103 (2.4*0.8),
104 (2.79*0.9), 105 (3*1).

The PDF text extraction loses which tubing sizes belong to which pump head (the
original is a merged-cell table): read that pairing in the PDF itself. This
project never writes registers 1000 and 1001.

## Appendix 1: CRC check C language code

CRC generation process:

1. Put one 16 bits register into hexadecimal FFFF (all 1), we call it CRC
   register.
2. XOR the first 8 bits byte of the message with the low byte of the 16 bits
   CRC register, the result is put in the CRC register.
3. Move the CRC register 1 bit to the right, MSB zeroing. Extract and check the
   LSB.
4. If the LSB is 0: repeat step 3 (another shift). If the LSB is 1: XOR the CRC
   register with the polynomial value 0xA001 (1010 0000 0000 0001).
5. Repeat steps 3 and 4 until 8 shifts are done. This completes the operation
   for one byte.
6. Repeat steps 2 to 5 for the next byte of the message, until the whole
   message is processed.
7. The final content of the CRC register is the CRC value.
8. When the CRC value is put in the message, the high and low bytes must be
   exchanged.
