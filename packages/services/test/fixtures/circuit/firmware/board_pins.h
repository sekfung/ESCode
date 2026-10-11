/*
 * ESCode G3 手工固件样例（本仓库原创，无第三方许可问题，见 fixtures/circuit/README.md）。
 * CubeMX 风格的引脚宏，与同目录的 board.dts / board.ioc 对照：
 *   - I2C1_SDA 在这里落在 PB8，而 board.dts 声明在 PB7（外设复用冲突）；
 *   - I2C1_SCL 与 board.dts 一致，都在 PB8（原理图上没有 PB8）。
 */

/* Define to prevent recursive inclusion -------------------------------------*/
#ifndef BOARD_PINS_H
#define BOARD_PINS_H

/* 与 board.dts 的 led0 一致：PB6 -> LED0 */
#define LED0_PIN GPIO_PIN_6
#define LED0_GPIO_Port GPIOB

/* 与 board.dts 的 uart2 tx-gpios 一致：PA2 -> UART2_TX */
#define UART2_TX_PIN GPIO_PIN_2
#define UART2_TX_GPIO_Port GPIOA
#define UART2_TX_PORT GPIOA

/* 有意不一致：I2C1_SDA 在 PB8（board.dts 是 PB7） */
#define I2C1_SDA_PIN GPIO_PIN_8
#define I2C1_SDA_GPIO_Port GPIOB

/* 有意不一致：I2C1_SCL 在 PB8（原理图上没有 PB8） */
#define I2C1_SCL_PIN LL_GPIO_PIN_8
#define I2C1_SCL_PORT GPIOB

#endif /* BOARD_PINS_H */
