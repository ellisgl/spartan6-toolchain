// Shared test helpers for the Mojo V3 (50 MHz clock, AVR serial bridge at
// 500 kbaud 8N1). Used by the UART-driven hardware tests.

// The AVR raises cclk once it is ready to forward FPGA serial data.
module mojo_ready (
    input  wire clk,
    input  wire cclk,
    output wire ready
);
    reg [1:0]  cclk_s = 0;
    reg [10:0] cnt = 0;
    assign ready = cnt[10];
    always @(posedge clk) begin
        cclk_s <= {cclk_s[0], cclk};
        if (!cclk_s[1])
            cnt <= 0;
        else if (!ready)
            cnt <= cnt + 1'b1;
    end
endmodule

module uart_rx #(parameter CLK_PER_BIT = 100) (
    input  wire       clk,
    input  wire       rx_pin,
    output reg  [7:0] data = 0,
    output reg        valid = 0
);
    reg [1:0] s = 2'b11;
    always @(posedge clk) s <= {s[0], rx_pin};
    wire rx = s[1];

    reg       busy = 0;
    reg [6:0] cnt = 0;
    reg [3:0] bitn = 0;
    reg [7:0] shift = 0;
    always @(posedge clk) begin
        valid <= 1'b0;
        if (!busy) begin
            if (!rx) begin
                busy <= 1'b1;
                cnt  <= CLK_PER_BIT / 2;
                bitn <= 0;
            end
        end else if (cnt != 0) begin
            cnt <= cnt - 1'b1;
        end else begin
            cnt <= CLK_PER_BIT - 1;
            if (bitn == 0) begin
                if (rx) busy <= 1'b0;
                bitn <= 1;
            end else if (bitn <= 8) begin
                shift <= {rx, shift[7:1]};
                bitn <= bitn + 1'b1;
            end else begin
                busy  <= 1'b0;
                data  <= shift;
                valid <= 1'b1;
            end
        end
    end
endmodule

// While `send` is high, takes `data` as soon as it is idle and the AVR is not
// signalling avr_rx_busy (its buffer is full; without that check back-to-back
// bytes get dropped), and pulses `taken` for one clock when it does. Callers
// hold `data` until `taken`.
// GAP_BITS of extra idle time follow each byte: the Mojo's AVR occasionally
// loses a byte when they arrive back to back at full speed.
module uart_tx #(parameter CLK_PER_BIT = 100, parameter GAP_BITS = 0) (
    input  wire       clk,
    input  wire       avr_busy,
    input  wire       send,
    input  wire [7:0] data,
    output reg        taken = 0,
    output wire       tx_pin
);
    reg       active = 0;
    reg [6:0] cnt = 0;
    reg [4:0] bitn = 0;
    reg [9:0] shift = 10'h3ff;
    reg [1:0] avr_busy_s = 2'b11;
    always @(posedge clk) avr_busy_s <= {avr_busy_s[0], avr_busy};
    assign tx_pin = shift[0];
    always @(posedge clk) begin
        taken <= 1'b0;
        if (!active) begin
            if (send && !taken && !avr_busy_s[1]) begin
                taken  <= 1'b1;
                shift  <= {1'b1, data, 1'b0};
                active <= 1'b1;
                cnt    <= CLK_PER_BIT - 1;
                bitn   <= 0;
            end
        end else if (cnt != 0) begin
            cnt <= cnt - 1'b1;
        end else begin
            cnt <= CLK_PER_BIT - 1;
            shift <= {1'b1, shift[9:1]};
            if (bitn == 9 + GAP_BITS)
                active <= 1'b0;
            bitn <= bitn + 1'b1;
        end
    end
endmodule
