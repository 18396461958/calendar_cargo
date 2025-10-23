cargo build --release
arm-none-eabi-objcopy -O binary -S target/thumbv7m-none-eabi/release/gpio_output target/thumbv7m-none-eabi/release/gpio_output.bin
sudo stm32flash -w target/thumbv7m-none-eabi/release/gpio_output.bin -v -g 0x08000000 /dev/ttyCH341USB0 -b 9600