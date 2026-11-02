// Carry-chain test over the Mojo's USB serial port: receives two 32-bit
// little-endian numbers a and b (8 bytes) and replies with 13 bytes:
//   a + b (4 bytes LE), a - b (4 bytes LE), a * 3 + 1 (4 bytes LE),
//   flags: bit 0 a < b (unsigned), bit 1 a == b, bit 2 $signed(a) < $signed(b),
//          bit 3 carry out of a + b, bits 7:4 = 4'b1010
// Checked by tools/alu-test.py. Synthesized with carry chains (CARRY4).
module alu (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));

    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [63:0] in = 0;      // a = in[31:0], b = in[63:32]
    reg [3:0]  nin = 0;
    reg [103:0] out = 0;
    reg [3:0]  nout = 0;    // bytes left to send
    reg        computing = 0;

    wire [31:0] a = in[31:0], b = in[63:32];
    wire [32:0] sum = {1'b0, a} + {1'b0, b};
    wire [31:0] diff = a - b;
    wire [31:0] mul3 = a * 3 + 1;
    wire [7:0]  flags = {4'b1010, sum[32], $signed(a) < $signed(b), a == b, a < b};

    wire tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready && !computing),
                 .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    // Timeout: a partial message is dropped after ~1.3 ms of silence.
    reg [15:0] idle = 0;

    always @(posedge clk) begin
        if (rx_valid) begin
            in   <= {rx_data, in[63:8]};
            nin  <= nin + 1'b1;
            idle <= 0;
        end else if (nin != 0 && !computing) begin
            idle <= idle + 1'b1;
            if (&idle) nin <= 0;
        end
        if (nin == 8 && !rx_valid) begin
            nin <= 0;
            computing <= 1'b1;
        end
        if (computing) begin
            out  <= {flags, mul3, diff, sum[31:0]};
            nout <= 13;
            computing <= 1'b0;
        end else if (tx_taken) begin
            out  <= {8'h00, out[103:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = in[7:0];
endmodule
