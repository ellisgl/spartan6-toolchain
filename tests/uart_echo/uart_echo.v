// Mojo V3 UART echo: replies to every byte received from the USB serial port
// with that byte + 1, and shows the last received byte on the LEDs.
//
// The Mojo's AVR bridges USB serial to the FPGA at 500 kbaud 8N1:
// FPGA RX on avr_tx (P55), FPGA TX on avr_rx (P59). The AVR raises cclk (P70)
// once it is ready; the FPGA waits for cclk to stay high ~1000 clocks before
// transmitting (as the Mojo base project's cclk_detector does).
// Until then avr_rx is tristated, as in the base project.
module uart_echo (
    input  wire       clk,     // 50 MHz
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,  // AVR TX -> FPGA RX
    output wire       avr_rx,  // FPGA TX -> AVR RX
    output wire [7:0] led
);
    localparam CLK_PER_BIT = 100;  // 50 MHz / 500 kbaud

    // --- AVR ready: cclk high for 1024 consecutive clocks ---
    reg [1:0]  cclk_s = 0;
    reg [10:0] cclk_cnt = 0;
    wire ready = cclk_cnt[10];
    always @(posedge clk) begin
        cclk_s <= {cclk_s[0], cclk};
        if (!cclk_s[1])
            cclk_cnt <= 0;
        else if (!ready)
            cclk_cnt <= cclk_cnt + 1'b1;
    end

    // --- receiver ---
    reg [1:0] rx_s = 2'b11;
    always @(posedge clk) rx_s <= {rx_s[0], avr_tx};
    wire rx = rx_s[1];

    reg       rx_busy = 0;
    reg [6:0] rx_cnt = 0;   // clocks within a bit
    reg [3:0] rx_bit = 0;   // 0 = start, 1..8 = data, 9 = stop
    reg [7:0] rx_shift = 0;
    reg [7:0] rx_data = 0;
    reg       rx_valid = 0;

    always @(posedge clk) begin
        rx_valid <= 1'b0;
        if (!rst_n) begin
            rx_busy <= 1'b0;
        end else if (!rx_busy) begin
            if (!rx) begin               // start bit edge
                rx_busy <= 1'b1;
                rx_cnt  <= CLK_PER_BIT / 2;  // sample mid-bit
                rx_bit  <= 0;
            end
        end else if (rx_cnt != 0) begin
            rx_cnt <= rx_cnt - 1'b1;
        end else begin
            rx_cnt <= CLK_PER_BIT - 1;
            if (rx_bit == 0) begin
                if (rx) rx_busy <= 1'b0;     // false start
                rx_bit <= 1;
            end else if (rx_bit <= 8) begin
                rx_shift <= {rx, rx_shift[7:1]};  // LSB first
                rx_bit <= rx_bit + 1'b1;
            end else begin                   // stop bit
                rx_busy  <= 1'b0;
                rx_data  <= rx_shift;
                rx_valid <= 1'b1;
            end
        end
    end

    // --- transmitter: one byte buffer ---
    reg       tx_busy = 0;
    reg [6:0] tx_cnt = 0;
    reg [3:0] tx_bit = 0;
    reg [9:0] tx_shift = 10'h3ff;
    reg       pending = 0;
    reg [7:0] pending_data = 0;

    always @(posedge clk) begin
        if (rx_valid) begin
            pending      <= 1'b1;
            pending_data <= rx_data + 1'b1;
        end
        if (!rst_n) begin
            tx_busy  <= 1'b0;
            tx_shift <= 10'h3ff;
            pending  <= 1'b0;
        end else if (!tx_busy) begin
            if (pending && ready && !rx_valid) begin
                tx_shift <= {1'b1, pending_data, 1'b0};  // stop, data, start
                tx_busy  <= 1'b1;
                tx_cnt   <= CLK_PER_BIT - 1;
                tx_bit   <= 0;
                pending  <= 1'b0;
            end
        end else if (tx_cnt != 0) begin
            tx_cnt <= tx_cnt - 1'b1;
        end else begin
            tx_cnt <= CLK_PER_BIT - 1;
            tx_shift <= {1'b1, tx_shift[9:1]};
            if (tx_bit == 9)
                tx_busy <= 1'b0;
            tx_bit <= tx_bit + 1'b1;
        end
    end

    assign avr_rx = ready ? tx_shift[0] : 1'bz;
    assign led = rx_data;
endmodule
