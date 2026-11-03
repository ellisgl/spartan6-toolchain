// SRLC32E using both Q (a tap) and Q31 (the cascade into a second SRLC32E),
// 8 bit-lanes, over the Mojo's serial port:
//   'A' a -> 'K'     tap address = a[4:0]
//   'S' d -> 2 bytes  shift d in, reply {chain end (64 deep), tap} as they
//                     were before this shift.
// The tap reads stage a of the first SRL (0 = newest); the chain end is
// stage 63 of the pair. Checked by tools/srlq31-test.py.
module srlq31 (
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

    reg  [4:0] a = 0;
    reg  [7:0] d = 0;
    reg        ce = 0;
    wire [7:0] tap, mid, last;
    genvar i;
    generate for (i = 0; i < 8; i = i + 1) begin : lane
        SRLC32E #(.INIT(32'h0)) s0 (.CLK(clk), .CE(ce), .D(d[i]), .A(a), .Q(tap[i]), .Q31(mid[i]));
        SRLC32E #(.INIT(32'h0)) s1 (.CLK(clk), .CE(ce), .D(mid[i]), .A(5'd31), .Q(last[i]), .Q31());
    end endgenerate

    reg        have_op = 0;
    reg [7:0]  op = 0;
    reg [15:0] out = 0;
    reg [1:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        ce <= 1'b0;
        if (rx_valid) begin
            // Unknown bytes are ignored (stray AVR bytes at startup).
            if (!have_op) begin
                if (rx_data == "A" || rx_data == "S") begin op <= rx_data; have_op <= 1'b1; end
            end else begin
                have_op <= 1'b0;
                if (op == "A") begin
                    a <= rx_data[4:0]; out <= "K"; nout <= 1;
                end else begin
                    out <= {last, tap}; nout <= 2; d <= rx_data; ce <= 1'b1;
                end
            end
        end else if (tx_taken) begin
            out <= {8'h00, out[15:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = tap;
endmodule
