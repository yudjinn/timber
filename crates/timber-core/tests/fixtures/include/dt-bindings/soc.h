#ifndef _DT_BINDINGS_SOC_H
#define _DT_BINDINGS_SOC_H

#define GIC_SPI 0
#define GIC_PPI 1
#define IRQ_TYPE_LEVEL_HIGH 4
#define GPIO_ACTIVE_LOW 1
#define CLK_UART0 12
#define CLK_I2C(n) (40 + (n))
#define PIN(bank, idx) (((bank) << 5) | (idx))
#define RK_FUNC_GPIO 0

#endif
