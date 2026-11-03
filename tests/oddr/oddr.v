// ODDR2 (clock forwarding / DDR output) on the board. The ODDR2 drives header
// pin P51 (bidirectional pad, output always enabled); the pin is read back
// through the same pad's input buffer and sampled on both edges of the clock.
//   'S' x -> 'K'  D0 = x[0], D1 = x[1] (the ODDR2's two halves; C0 = clk,
//                 C1 = ~clk)
//   'C'   -> 8 bytes (LE): in 65536 cycles of the 50 MHz clock, how often the
//                 pin read 1 when sampled on the clock's falling edge, and on
//                 its rising edge. The output changes on each edge and arrives
//                 a few ns later, so the falling-edge sample sees D0 and the
//                 rising-edge sample D1.
// Checked by tools/oddr-test.py.
module oddr (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    inout  wire       pad,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg d0 = 1'b1, d1 = 1'b0;
    wire q, back;
    ODDR2 #(.DDR_ALIGNMENT("NONE"), .INIT(1'b0), .SRTYPE("SYNC")) od (
        .Q(q), .C0(clk), .C1(~clk), .CE(1'b1), .D0(d0), .D1(d1), .R(1'b0), .S(1'b0));
    IOBUF iob (.IO(pad), .I(q), .T(1'b0), .O(back));

    // The pin sampled as data on both clock edges.
    reg sp = 0, sn = 0;
    always @(posedge clk) sp <= back;
    always @(negedge clk) sn <= back;

    reg [16:0] wcnt = 0;
    reg        window = 0, report = 0;
    reg [9:0]  settle = 0;
    reg [31:0] ones_n = 0, ones_p = 0;
    reg        have_op = 0;
    reg [63:0] out = 0;
    reg [3:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid) begin
            // Unknown bytes are ignored (stray AVR bytes at startup).
            if (have_op) begin
                have_op <= 1'b0;
                d0 <= rx_data[0]; d1 <= rx_data[1];
                out <= "K"; nout <= 1;
            end else if (rx_data == "S") begin
                have_op <= 1'b1;
            end else if (rx_data == "C" && !window && settle == 0) begin
                window <= 1'b1; wcnt <= 0; ones_n <= 0; ones_p <= 0;
            end
        end
        if (window) begin
            wcnt <= wcnt + 1'b1;
            ones_n <= ones_n + sn;
            ones_p <= ones_p + sp;
            if (wcnt == 17'd65535) begin window <= 1'b0; settle <= 1; end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;
            if (&settle) begin settle <= 0; report <= 1'b1; end
        end
        if (report) begin
            out <= {ones_p, ones_n}; nout <= 8;
        end else if (tx_taken && !(rx_valid && have_op)) begin
            out <= {8'h00, out[63:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {6'b0, d1, d0};
endmodule
