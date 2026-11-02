// Mojo V3 tristate test, driven over the USB serial port (500 kbaud 8N1 via
// the AVR, as in tests/uart_echo).
//
// - avr_rx (FPGA TX) is tristated until the AVR is ready (OBUFT), as the
//   Mojo base project does.
// - Two header pins are bidirectional (IOBUF): pa has a pull-up, pb a
//   pull-down. Each received byte sets them:
//       bit 0 = pa output enable, bit 1 = pa value,
//       bit 2 = pb output enable, bit 3 = pb value
//   and, a few clocks later, the FPGA replies with what the pins read back:
//       {4'b1010, 2'b00, pb, pa}
//   A driven pin reads its own value; an undriven one reads its pull.
//   Nothing must be connected to pa/pb (see pins.ucf).
module tristate (
    input  wire       clk,     // 50 MHz
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,  // AVR TX -> FPGA RX
    output wire       avr_rx,  // FPGA TX -> AVR RX (tristate until ready)
    inout  wire       pa,
    inout  wire       pb,
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
    reg [6:0] rx_cnt = 0;
    reg [3:0] rx_bit = 0;
    reg [7:0] rx_shift = 0;
    reg [7:0] rx_data = 0;
    reg       rx_valid = 0;

    always @(posedge clk) begin
        rx_valid <= 1'b0;
        if (!rst_n) begin
            rx_busy <= 1'b0;
        end else if (!rx_busy) begin
            if (!rx) begin
                rx_busy <= 1'b1;
                rx_cnt  <= CLK_PER_BIT / 2;
                rx_bit  <= 0;
            end
        end else if (rx_cnt != 0) begin
            rx_cnt <= rx_cnt - 1'b1;
        end else begin
            rx_cnt <= CLK_PER_BIT - 1;
            if (rx_bit == 0) begin
                if (rx) rx_busy <= 1'b0;
                rx_bit <= 1;
            end else if (rx_bit <= 8) begin
                rx_shift <= {rx, rx_shift[7:1]};
                rx_bit <= rx_bit + 1'b1;
            end else begin
                rx_busy  <= 1'b0;
                rx_data  <= rx_shift;
                rx_valid <= 1'b1;
            end
        end
    end

    // --- pins under test ---
    reg [3:0] ctl = 4'b0000;  // all released at start-up
    always @(posedge clk)
        if (rx_valid) ctl <= rx_data[3:0];
    assign pa = ctl[0] ? ctl[1] : 1'bz;
    assign pb = ctl[2] ? ctl[3] : 1'bz;
    reg [1:0] pa_s = 0, pb_s = 0;
    always @(posedge clk) begin
        pa_s <= {pa_s[0], pa};
        pb_s <= {pb_s[0], pb};
    end

    // Reply 32 clocks after the byte arrives (lets the pins settle).
    reg [5:0] wait_cnt = 0;
    reg       waiting = 0;
    reg       pending = 0;
    reg [7:0] pending_data = 0;
    always @(posedge clk) begin
        if (rx_valid) begin
            waiting  <= 1'b1;
            wait_cnt <= 0;
        end else if (waiting) begin
            wait_cnt <= wait_cnt + 1'b1;
            if (&wait_cnt) waiting <= 1'b0;
        end
    end
    wire reply_now = waiting && &wait_cnt;

    // --- transmitter: one byte buffer ---
    reg       tx_busy = 0;
    reg [6:0] tx_cnt = 0;
    reg [3:0] tx_bit = 0;
    reg [9:0] tx_shift = 10'h3ff;

    always @(posedge clk) begin
        if (reply_now) begin
            pending      <= 1'b1;
            pending_data <= {4'b1010, 2'b00, pb_s[1], pa_s[1]};
        end
        if (!rst_n) begin
            tx_busy  <= 1'b0;
            tx_shift <= 10'h3ff;
            pending  <= 1'b0;
        end else if (!tx_busy) begin
            if (pending && ready && !reply_now) begin
                tx_shift <= {1'b1, pending_data, 1'b0};
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
    assign led = {ctl, 2'b00, pb_s[1], pa_s[1]};
endmodule
