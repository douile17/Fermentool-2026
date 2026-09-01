

### Baoding Shenchen Precision Pump Co., Ltd. 

Add: No. 103, Building 2, Pivot Industrial Park, Fuxing East Raod, Baoding, China. Tel: 0086-312-6780681    Fax: 0086-312-6780636 Website: <u>www.good-pump.com   Email: amy@good-pump.com</u> 

## **LabQ MODBUS Communication Protocol** 

**Note: The hexadecimal numbers are expressed by ‘XXXXH’ or ‘XXH’ in the below description.** 

#### **1. MODBUS-RTU standard communication format** 

This communication use MODBUS RTU mode, message frame as below: 

|Slave address|Function code|Data area|CRC Check(Cyclic Redundancy Check)|
|---|---|---|---|
|1 Byte|1 Byte|0 or up to 252 bytes|2 Bytes|
||||CRC low<br>CRC high|



- (1) **Slave address** : Host control peristaltic pump address No. . The pump address No. should not be same when they are in the same 485 line. The address No. range is 1~247, 0 means broadcast. 

- (2) **Function code** : The protocol use 2 common function codes which defined by MODBUS protocol. 

   - **03H** : Read the contents of holding register 

   - **06H** : Write a word to the holding register 

   - **10H** : Write a long type to the data zone of holding register: Specific information instructions that peristaltic pump needs to execute, such as start/stop, direction, accelerate/decelerate etc. 

- (3) **CRC check** : CRC code is 2 bytes, 16 check codes. Use CRC-16 (which used in American binary synchronous system). 

Polynomial: G(X)=X16+X15+X2+1 **.** 

**CRC check C language code please refer to Appendix 1** . 

#### **2. Communication Setting** 

- **(1) Communication baud rate: 1200, 2400, 4800, 9600 optional** 

- **(2) Byte structure:** 1 start bit + 8 data bits +1 even parity bit + 1 stop bit 

- **(3) Bit sequence sending order:** The least significant bit ( **LSB** )….. The most significant bit ( **MSB** ) 

|**Start**|**1**<br>**2**<br>**3**<br>**4**<br>**5**<br>**6**<br>**7**<br>**8**<br>**Check**|**Stop**|
|---|---|---|



- **(4) Data transferring format:** 

**Integer (2 bytes):** 

Data: (High bit) The second byte     The first byte(Low bit) Send: The second byte     The first byte For example:  1234H  send 12H   34H 

**Floating-point type (4 bytes):** 

Data: (High bit) The fourth byte   The third byte   The second byte   The first byte (Low bit) Send: The fourth byte   The third byte   The second byte   The first byte For example: 8.9  send 41H  0EH  66H   66H 

- 1 - 





# P <u><mark>EE</mark></u> ~~<mark>TTP</mark> —~~ 



Baoding Shenchen Precision Pump Co., Ltd. Add: No. 103, Building 2, Pivot Industrial Park, Fuxing East Raod, Baoding, China. Tel: 0086-312-6780681    Fax: 0086-312-6780636 

Website: <u>www.good-pump.com   Email: amy@good-pump.com</u> 

|06H|Slave(peristaltic pump)<br>busy|<br>The current state of the peristaltic pump conflict with<br>the command received, unable to complete the<br>command.|
|---|---|---|



**Peristaltic Pump only receive MODBUS command with the Main Interface, other interface do not receive message.** 

#### **5. Holding register address and contents** 

##### **Basic Parameters Setting** 

|Address<br>(Decimal)|Name|Range|Data Type|
|---|---|---|---|
|1000|Pumphead type|Refer to chart 1 <Number of pump|unsigned short int (2 Bytes)|
|1001|Tubingsize|head & tubing>|unsigned short int (2 Bytes)|
|1002|Motor speed|0.1-350rpm|float          (4 Bytes)|
|1004|Flow rate|0-99999mL|Float|
|1006|Start/stop<br>control|1: Start   0: Stop|unsigned short int (2 Bytes)|
|1007|Direction<br>control|1: clockwise   0: counterclockwise|unsigned short int (2 Bytes)|
|1008|Full speed to<br>run|1: start full speed  0: stop full speed|unsigned short int (2 Bytes)|
|1009|Back<br>suction<br>angle|0-360°|unsigned short int (2 Bytes)|



**Note: Please set register parameters according to the chart, multiple registers are set continuously** 

##### **without receiving an instruction.** 

#### **6. Sending Data format** 

##### **Unsigned short int format** 

|Peristaltic<br>pumpaddress|Function<br>Code|Regist|er address|Da<br>(unsigned|ta<br>short int)|C|RC|
|---|---|---|---|---|---|---|---|
||06H|Addre<br>ss H|Address L|Data H|Data L|L|H|



- 3 - 



Baoding Shenchen Precision Pump Co., Ltd. Add: No. 103, Building 2, Pivot Industrial Park, Fuxing East Raod, Baoding, China. Tel: 0086-312-6780681    Fax: 0086-312-6780636 

Website: <u>www.good-pump.com   Email: amy@good-pump.com</u> 

|**Float fo**|**rmat**|||||||
|---|---|---|---|---|---|---|---|
|Pump<br>addre<br>ss|Function<br>Code|Register<br>address|The number<br>of register|The<br>number of<br>byte|Data<br>(Float)||CRC check|
||10H|H<br>L|00H<br>02H|04H|L1<br>L2<br>H1|H2|L<br>H|



##### **(1) Set pump head type** 

The peristaltic pump address is 1, set the pump head to KT15, the number is 0000H. 

Send: **01 06 03 E8 00 00 09 BA** 

Back: **01 06 03 E8 00 00 09 BA** 

##### **(2) Set tubing size** 

The peristaltic pump address is 1, set the tubing type to 16#, the number is 0010H. 

Send: **01 06 03 E9 00 10 59 B6** 

Back: **01 06 03 E9 00 10 59 B6** 

##### **(3) Set motor speed** 

The peristaltic pump address is 1, set the motor speed to 58.8rpm. 

Send: **01 10 03 EA 00 02 04 42 6B 33 33 58 29** 

Back: **01 10 03 EA 00 02 60 78** 

##### **(4) Set flow rate** 

The peristaltic pump address is 1, set the flow rate to 50ml/min 

Send: **01 10 03 EC 00 02 04 42 48 00 00 7D 2C** 

Back: **01 10 03 EC 00 02 80 79** 

##### **(5) Control start/stop** 

The peristaltic pump address is 1, control start is 0001H, control stop is 0000H. 

Send start: **01 06 03 EE 00 01 28 7B** 

Send stop: **01 06 03 EE 00 01 28 7B** 

##### **(6) Direction control** 

The peristaltic pump address is 1, clockwise is 0001H, counterclockwise is 0000H. 

Send: **01 06 03 EF 00 01 79 BB** (clockwise) 

- 4 - 



Baoding Shenchen Precision Pump Co., Ltd. Add: No. 103, Building 2, Pivot Industrial Park, Fuxing East Raod, Baoding, China. Tel: 0086-312-6780681    Fax: 0086-312-6780636 Website: <u>www.good-pump.com   Email: amy@good-pump.com</u> 

##### Back: **01 06 03 EF 00 01 79 BB** 

##### **(7) Set full speed** 

The peristaltic pump address is 1, the full speed is 0001H. 

Send: **01 06 03 F0 00 01 48 7D** 

Back: **01 06 03 F0 00 01 48 7D** 

##### **(8) Set back suction angle** 

The peristaltic pump address is 1, set back suction angle to 100° 

Send: **01 06 03 F1 00 64 D9 96** 

Back: **01 06 03 F1 00 64 D9 96** 

Chart 1 Number of Pump head & tubing 

|**Pump head**|**Head Type**|**Tubing type**|**Tubing size**|
|---|---|---|---|
|||13|13#|
|||14|14#|
|KT15|0|19|19#|
|||16|16#|
|||25|25#|



#### —— **7. Appendix 1 CRC Check C Language Code** 

#### **CRC generation process:** 

1. Put one 16 bits register into hexadecimal FFFF( all 1), we call it CRC register. 

2. Make the first 8 bytes with 16 CRC register low bytes XOR, the result put in CRC register. 

3. Detect LSB of CRC register 

If LSB is 0, move CRC register 1 bit to right (towards to direction of LSB) , MSB zeroing. 

If LSB is 1, move CRC register 1 bit to right (towards to direction of LSB), MSB zeroing, then XOR 

the polynomial value of the CRC register 0xA001 (1010 0000 0000 0001). 

4. Repeat Step 3, until finish 8 shifts. After finish this operation, will finish the complete operation for 8 Bytes. 

5. Repeat Step 2 to Step 5 for the next Bytes in message. Continue this operation till all the message 

- 5 - 



Baoding Shenchen Precision Pump Co., Ltd. Add: No. 103, Building 2, Pivot Industrial Park, Fuxing East Raod, Baoding, China. Tel: 0086-312-6780681    Fax: 0086-312-6780636 Website: <u>www.good-pump.com   Email: amy@good-pump.com</u> 

be deal with finished. 

6. The final content in CRC register is CRC value. 

7. When put CRC value in message, high and low Bytes must be exchanged, described as below: 

C language code: 

void CRCVerify(char *rec,char CRClen,char CRCdata[2]) 

{ 

char i1,j; 

unsigned int crc_data=0xffff; 

for(i1=0; i1<CRClen; i1++) 

{ crc_data=crc_data^rec[i1]; 

for(j=0; j<8; j++) { if(crc_data&0x0001) { crc_data>>=1; crc_data^=0xA001; } else { crc_data>>=1; } } } CRCdata[0]=(char)(crc_data); CRCdata[1]=(char)(crc_data>>8); 

} 

- 6 - 

