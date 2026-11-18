<u>Innofluid Co., Ltd.</u> 



# **Standard MODBUS Communication Protocol** 

**Note: The hexadecimal numbers are expressed by ‘XXXXH’ or ‘XXH’ in the below description.** 

### **1. MODBUS-RTU Standard Communication Format** 

This communication uses MODBUS RTU mode, message frame as below: 

|Slave address|Function code|Data area|CRC Check (Cyclic Redundancy Check)|
|---|---|---|---|
|1 Byte|1 Byte|0 or up to 252 bytes|2 Bytes|
||||CRC low<br>CRC high|



- (1) **Slave address** : Host controls address No. of peristaltic pump controller. The pump address No. should not be same 

when they are in the same 485 line. The address No. range is 1~32, 0 means broadcast. 

- (2) **Function code** : The protocol uses 2 common function codes which defined by MODBUS protocol. 

**03H** : Read the contents of holding register 

**06H** : Write a word to the holding register 

**10H** : Write a long type to holding register 

- **(3) Data zone:** Specific information instructions that the peristaltic pump needs to execute, such as start/stop, direction, 

accelerate/decelerate etc. 

- (4) **CRC check** : CRC code is 2 bytes, 16 check codes. Use CRC-16(which used in American binary synchronous system). 

Polynomial: G(X)=X16+X15+X2+1 **.** 

**CRC check C language code please refer to Appendix 1** . 

### **2. Communication Setting** 

- **(1) Communication baud rate: 1200, 2400, 4800, 9600 optional** 

- (2) **Byte structure:** 1 start bit + 8 data bits +1 even parity bit + 1 stop bit 

- **(3) Bit sequence sending order:** The least significant big( **LSB** )….. The most significant bit ( **MSB** ) 

|**Start**<br>**1**<br>**2**<br>**3**<br>**4**<br>**5**<br>**6**<br>**7**<br>**8**<br>**Check**|**Stop**|
|---|---|



- **(4) Data transferring format:** 

### **Integer (2 bytes):** 

**Data** : The second byte The first byte **Send** : The second byte The first byte **For example** : 1234H send 12H 34H 

### **Floating-point type (4 bytes):** 

**Data** : The fourth byte The third byte The second byte The first byte 

- 1 - 



## <u>Innofluid Co., Ltd.</u> 

**Send** : The fourth byte The third byte The second byte The first byte **For example** : 12345678H send 12H 34H 56H 78H 

### **3. MODBUS Message RTU Frame Format** 

In RTU mode, the message frames are distinguished by idle intervals with a duration of at least 3.5 characters. As the follow 

picture: 



<!-- Start of picture text -->
A Modbus message ______,<br>p Adds [TeData|<br>>23.5 8Bits; | 8 Bits;, Nx 8 Bits: 16 BitsP 23.5=<br>characters characters<br><!-- End of picture text -->

**The entire message frame must be sent in a continuous stream of characters. If the idle space between two characters** 

**is greater than 1.5 character times, the message frame is considered as incomplete, should be discarded by receiving** 

### **node. As below:** 



<!-- Start of picture text -->
Frame | normal Frame 2 abnormal<br>><br>rotttd H H<br>Thoth tt H H<br>tenih e ott =Lame<br>1.5 characters > 15 characters<br><!-- End of picture text -->

### **4. Abnormal Response** 

When host sending request data, the slave receives abnormal data, an abnormal response is required. If the address code sent 

from host is wrong, there is no this address code between slaves or the data received by slave is wrong when CRC check, no abnormal code return, the host should have super response mechanism. 

**Function code domain** : Abnormal response function code is normal response function code +80H. 

**Data domain** : Return to abnormal code, define as below: 

**Chart 1** : Abnormal code definition 

- 2 - 



## <u>Innofluid Co., Ltd.</u> 

|**Code**|**Name**|**Meaning**|
|---|---|---|
|01H|Illegal function code|The<br>function<br>code<br>received<br>by<br>peristaltic<br>pump<br>except<br>03H/06H/10H.|
|02H|Illegal data address|This abnormal code means the register address is not allowed<br>data which received by peristaltic pump.|
|03H|Illegal data value|Written data does not meet the operating range.|
|06H|Slave (peristaltic pump)|The current state of the peristaltic pump conflicts with the|
||busy|command received, unable to complete the command.|



- **Peristaltic Pump only receives MODBUS command with the Main Interface, other interfaces do not receive message.** 

- **Only the unit set to independent operation receives Modbus command, the unit of logical operation does not receive command.** 

### **5. The Address and Content of Holding Register** 

|Address (decimal)|Name|Range|Data Type|
|---|---|---|---|
|0010|All start|1: starts<br>0: stops|unsigned short int (2 Bytes)|



Note: This register address is all start/stop for all independent operation unit which in a certain controller’s address. 

**Basic Parameters Setting** 

|Address<br>(Decimal)|Name|Range|Data Type|
|---|---|---|---|
|n000|Pump head type|The<br>corresponding<br>parameters<br>are<br>shown in Table 1|unsigned short int (2 Bytes)|
|n001|Tubing size|The<br>corresponding<br>parameters<br>are<br>shown in Table 1|unsigned short int (2 Bytes)|
|n002|Back suction angle|0-360°|unsigned short int (2 Bytes)|
|n003|Start/stop|1: start<br>0: stop|unsigned short int (2 Bytes)|
|n004|Direction|1: clockwise<br>0: counterclockwise|unsigned short int (2 Bytes)|
|n005|Operation mode|0: transferring<br>1: dispensing|unsigned short int (2 Bytes)|
|n006|Full speed running|1: start full speed<br>0: stop full speed|unsigned short int (2 Bytes)|
|n007|Full speed direction|1: clockwise<br>0: counterclockwise|unsigned short int (2 Bytes)|
|n008|Dispensing<br>groups<br>in dispensing mode|1-3|unsigned short int (2 Bytes)|
|n009|Repeat<br>number<br>in|0-9999|unsigned short int (2 Bytes)|



- 3 - 



## <u>Innofluid Co., Ltd.</u> 

||dispensing mode||||
|---|---|---|---|---|
|n010|Dispensing<br>liquid<br>volume|0.1-99999 mL|Float|(4 Bytes)|
|n012|Operation time|0.1-9999 (s)|Float|(4 Bytes)|
|n014|Pause time|0.5-9999 (s)|Float|(4 Bytes)|
|n016|Dispensing<br>liquid<br>volume|0.1-99999 mL|Float|(4 Bytes)|
|n018|Operation time|0.1-9999 (s)|Float|(4 Bytes)|
|n020|Pause time|0.5-9999 (s)|Float|(4 Bytes)|
|n022|Dispensing<br>liquid<br>volume|0.1-99999 mL|Float|(4 Bytes)|
|n024|Operation time|0.1-9999 (s)|Float|(4 Bytes)|
|n026|Pause time|0.5-9999 (s)|Float|(4 Bytes)|
|n028|Delayed start time<br>in dispensing mode|0-9999 min|float|(4 Bytes)|
|n030|Flow<br>rate<br>in<br>transferring mode|0.1-99999<br>mL/min|float|(4 Bytes)|
|n032|Speed<br>in<br>transferring mode|0.1-600rpm|float|(4 Bytes)|
|n034|Delayed start time<br>in transferring mode|0-9999 min|float|(4 Bytes)|



### **Note:** 

- (1) Full speed can only be enabled in transfer mode or when the unit stops. Otherwise return an error. 

- (2) Full direction can only be enabled in stop status. Otherwise return an error. 

- (3) Direction setting can only be enabled in transfer mode or when the unit stops. Otherwise return an error. 

- (4) Transfer parameters can be set only when the running mode is Transfer mode. 

- (5) Dispensing parameters can be set only when the running mode is dispensing mode. 

- (6) If dispensing parameter settings exceed the number of dispensing groups, the settings are invalid and an error is 

   - returned. For example: the dispensing group number is 1, this is invalid when setting any of the register addresses 

   - 1023-1033. 

- (7) Please set register parameters separately according to the table. Do not receive one instruction to set multiple registers continuously. 

- (8) The value range of n is 1-8, each represents 8 units of a controller. 

- (9) The startup delay of the dispensing mode and transfer mode is measured in minutes and cannot be changed. 

- 4 - 



## <u>Innofluid Co., Ltd.</u> 

Setting parameters in calibration 

|Address<br>(Decimal)|Name|Range|Data Type|
|---|---|---|---|
|n100|Test time|0.5-9999s|float (4 Bytes)|
|n102|Test start|1: start<br>0: stop|unsigned short int (2 Bytes)|
|n103|Actual filling volume|0-9999 mL|float<br>(4 Bytes)|
|n105|Restore to initialization|1: restore to calibration|unsigned short int (2 Bytes)|
|n106|Micro-adjust liquid volume|1: increase<br>0: decrease|unsigned short int (2 Bytes)|
|n107|Choose group number of dispensing|0-2|unsigned short int (2 Bytes)|



**Table 1 Pump head and tubing size** 

|**Pump head**|**Pump head**|**Tubing size**|**Tubing specification**|
|---|---|---|---|
|||13|13#|
|||14|14#|
|||19|19#|
|EasyPumpI/III|0|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|||15|15#|
|EasPumII/IV|1|24|24#|
|yp||35|35#|
|||36|36#|
|||13|13#|
|||14|14#|
|EasyPumpV/VI|2|19|19#|
|||16|16#|
|||25|25#|
|||13|13#|
|||14|14#|
|||19|19#|
|2*EasyPumpI/III|3|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|||15|15#|
|2*EasyPumpII/IV|4|24|24#|
|||35|35#|



- 5 - 



## <u>Innofluid Co., Ltd.</u> 

|||36|36#|
|---|---|---|---|
|||101|1*1|
|||102|2*1|
|||103|2.4*0.86|
|||104|2.79*0.86|
|||105|3*1|
|||106|0.19*0.86|
|AMC(10)|5|107|0.25*0.86|
|_||108|0.51*0.86|
|||109|0.89*0.86|
|||110|1.14*0.86|
|||111|1.42*0.86|
|||116|1.52*0.86|
|||112|2.06*0.86|
|||114|0.13*0.86|
|||101|1*1|
|||102|2*1|
|||103|2.4*0.86|
|||104|2.79*0.86|
|||105|3*1|
|||106|0.19*0.86|
|AMC_(6)|6|107|0.25*0.86|
|||108|0.51*0.86|
|||109|0.89*0.86|
|||110|1.14*0.86|
|||116|1.52*0.86|
|||111|1.42*0.86|
|||112|2.06*0.86|
|||114|0.13*0.86|
|||13|13#|
|||14|14#|
|HandyPump01(3)|7|19|19#|
|||16|16#|
|||25|25#|
|||13|13#|
|HandyPump01(6)|8|14|14#|
|||19|19#|
|||16|16#|



- 6 - 



## <u>Innofluid Co., Ltd.</u> 

|||25|25#|
|---|---|---|---|
|||13|13#|
|HandyPump02|9|14|14#|
|||19|19#|
|||16|16#|
|||13|13#|
|||14|14#|
|KT15|10|19|19#|
|||16|16#|
|||25|25#|
|||13|13#|
|||14|14#|
|MiniPump|11|19|19#|
|||16|16#|
|||25|25#|
|||13<br>14|13#<br>14#|
|||19|19#|
|YZ1515x|12|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|YZ2515x|13|15<br>24|15#<br>24#|
|||13|13#|
|||14|14#|
|||19|19#|
|2*YZ1515x|14|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|2*YZ2515x|15|15|15#|
|||24|24#|
|||101|1*1|
|||102|2*1|
|MCn(10)|16|105|3*1|
|||106|0.19*0.86|
|||107|0.25*0.86|



- 7 - 



## <u>Innofluid Co., Ltd.</u> 

|||108|0.51*0.86|
|---|---|---|---|
|||109|0.89*0.86|
|||110|1.14*0.86|
|||111|1.42*0.86|
|||116|1.52*0.86|
|||112|2.06*0.86|
|||103|2.4*0.8|
|||104|2.79*0.86|
|||101|1*1|
|||102|2*1|
|||105|3*1|
|||106|0.19*0.86|
|||107|0.25*0.86|
|||108|0.51*0.86|
|MCn(6)|17|109|0.89*0.86|
|||110|1.14*0.86|
|||111|1.42*0.86|
|||116|1.52*0.86|
|||112|2.06*0.86|
|||103|2.4*0.8|
|||104|2.79*0.86|
|||15|15#|
|DZ25-3L|18|24|24#|
|||35|35#|
|||36|36#|
|||15|15#|
|2*DZ25-3L|19|24|24#|
|||35|35#|
|||36|36#|
|||15|15#|
|DZ25-6L|20|24|24#|
|||35|35#|
|||36|36#|
|||15|15#|
|2* DZ25-6L|21|24|24#|
|||35|35#|
|||36|36#|
|YZ35|22|26|26#|



- 8 - 



## <u>Innofluid Co., Ltd.</u> 

|||73|73#|
|---|---|---|---|
|||82|82#|
|||26|26#|
|YZ35_2|23|73|73#|
|||82|82#|
|||13|13#|
|||14|14#|
|||19|19#|
|DY15|24|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|||15|15#|
|DY25|25|24|24#|
|||35|35#|
|||36|36#|
|SN15|26|14<br>16|14#<br>16#|
|SN25|27|24|24#|
|||16|16#|
|UD15|28|25|25#|
|||17|17#|
|||114|0.5*1.6|
|HandyPump-Y|29|13|13#|
|||14|14#|
|||19|19#|
|||114|0.5*1.6|
|||13|13#|
|EasyPumpV/VI-Y|30|14|14#|
|||19|19#|
|||16|16#|
|||13|13#|
|||14|14#|
|||19|19#|
|KD15(3)|31|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|



- 9 - 



## <u>Innofluid Co., Ltd.</u> 

|||13|13#|
|---|---|---|---|
|||14|14#|
|||19|19#|
|KD15(4)|32|16|16#|
|||25|25#|
|||17|17#|
|||18|18#|
|KD25(3)|33|15|15#|
|||24|24#|
|KD25(4)|34|15|15#|
|||24|24#|
|||26|26#|
|YZ35-II|35|73|73#|
|||82|82#|
|||26|26#|
|2*YZ35-II|36|73|73#|
|||82|82#|



### **6. Appendix 1——CRC Check C Language Code** 

### **CRC generation process:** 

- **1)** Put one 16 bits register into hexadecimal FFFF( all 1), we call it CRC register. 

- **2)** XOR the first 8-bit byte of the message with the low byte of the 16-bit CRC register, the result put in CRC register. 

- **3)** Move CRC register 1 bit to right (towards to direction of LSB), MSB zeroing. Extract and test LSB. 

- **4)** (If LSB is 0): Repeat step 3, (Another shift). 

(If LSB is 1): For CRC register XOR polynomial value 0xA001 (1010 0000 0000 0001). 

- **5)** Repeat Step 3 and Step 4, until finish 8 shifts. After finish this operation, will finish the complete operation for 8 Bytes. 

- **6)** Repeat Step 2 to Step 5 for the next Bytes in message. Continue this operation till all the message be deal with finished. 

- **7)** The final content in CRC register is CRC value. 

- **8)** When put CRC value in message, high and low bytes must be exchanged, described as below: 

void CRCVerify(char *rec,char CRClen,char CRCdata[2]) 

- 10 - 



## <u>Innofluid Co., Ltd.</u> 

{ char i1,j; unsigned int crc_data=0xffff; for(i1=0; i1<CRClen; i1++) { crc_data=crc_data^rec[i1]; for(j=0; j<8; j++) { if(crc_data&0x0001) { crc_data>>=1; crc_data^=0xA001; } else { crc_data>>=1; } } } CRCdata[0]=(char)(crc_data); CRCdata[1]=(char)(crc_data>>8); 

} 

- 11 - 

