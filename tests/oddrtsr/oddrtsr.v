// ODDR2 set / reset from logic, on the board: as tests/oddrt, plus
//   'R' x -> 'K'  x[0]: pa's data ODDR2 S and tristate ODDR2 R (driving 1);
//                 x[1]: pb's data ODDR2 R and tristate ODDR2 S (released: its
//                 pull-down)
// ISE requires an OLOGIC's data and tristate registers to share their
// set/reset signal (the site's one SR line); the kinds may differ.
// Checked by tools/oddrtsr-test.py.
// Tristated DDR outputs (ODDR2 data on an IOBUF with T in use), on the board.
// Two unconnected header pins (see pins.ucf):
//   pa (pull-up):   ODDR2 data, T from a second ODDR2, both halves alike
//   pb (pull-down): ODDR2 data, T from a second ODDR2, each half its own
// (ISE requires T from an ODDR2 when the data comes from one.)
// Each pin is read back through its own input buffer and sampled on both
// clock edges, as in tests/oddr.
//   'S' x -> 'K'  x[0] pa D0, x[1] pa D1, x[2] pa T,
//                 x[3] pb D0, x[4] pb D1, x[5] pb T0, x[6] pb T1
//   'C'   -> 16 bytes (LE): in 65536 cycles, how often pa read 1 on the
//            falling / rising edge, then the same for pb
// Checked by tools/oddrt-test.py.
module oddrtsr (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    inout  wire       pa,
    inout  wire       pb,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [6:0] x = 7'b0100100;   // both released
    reg [1:0] sr = 0;
    wire qa, qb, tb, back_a, back_b;
    ODDR2 #(.DDR_ALIGNMENT("NONE"), .INIT(1'b0), .SRTYPE("SYNC")) oda (
        .Q(qa), .C0(clk), .C1(~clk), .CE(1'b1), .D0(x[0]), .D1(x[1]), .R(1'b0), .S(sr[0]));
    wire ta;
    ODDR2 #(.DDR_ALIGNMENT("NONE"), .INIT(1'b1), .SRTYPE("SYNC")) ota (
        .Q(ta), .C0(clk), .C1(~clk), .CE(1'b1), .D0(x[2]), .D1(x[2]), .R(sr[0]), .S(1'b0));
    IOBUF ioa (.IO(pa), .I(qa), .T(ta), .O(back_a));

    ODDR2 #(.DDR_ALIGNMENT("NONE"), .INIT(1'b0), .SRTYPE("SYNC")) odb (
        .Q(qb), .C0(clk), .C1(~clk), .CE(1'b1), .D0(x[3]), .D1(x[4]), .R(sr[1]), .S(1'b0));
    // INIT 0 here (1 on pa's), so both encodings are checked against ISE.
    ODDR2 #(.DDR_ALIGNMENT("NONE"), .INIT(1'b0), .SRTYPE("SYNC")) otb (
        .Q(tb), .C0(clk), .C1(~clk), .CE(1'b1), .D0(x[5]), .D1(x[6]), .R(1'b0), .S(sr[1]));
    IOBUF iob (.IO(pb), .I(qb), .T(tb), .O(back_b));

    // The pins sampled as data on both clock edges.
    reg ap = 0, an = 0, bp = 0, bn = 0;
    always @(posedge clk) begin ap <= back_a; bp <= back_b; end
    always @(negedge clk) begin an <= back_a; bn <= back_b; end

    reg [16:0] wcnt = 0;
    reg        window = 0, report = 0;
    reg [9:0]  settle = 0;
    reg [31:0] a_n = 0, a_p = 0, b_n = 0, b_p = 0;
    reg        have_op = 0, have_r = 0;
    reg [127:0] out = 0;
    reg [4:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid) begin
            // Unknown bytes are ignored (stray AVR bytes at startup).
            if (have_r) begin
                have_r <= 1'b0;
                sr <= rx_data[1:0];
                out <= "K"; nout <= 1;
            end else if (have_op) begin
                have_op <= 1'b0;
                x <= rx_data[6:0];
                out <= "K"; nout <= 1;
            end else if (rx_data == "S") begin
                have_op <= 1'b1;
            end else if (rx_data == "R") begin
                have_r <= 1'b1;
            end else if (rx_data == "C" && !window && settle == 0) begin
                window <= 1'b1; wcnt <= 0; a_n <= 0; a_p <= 0; b_n <= 0; b_p <= 0;
            end
        end
        if (window) begin
            wcnt <= wcnt + 1'b1;
            a_n <= a_n + an; a_p <= a_p + ap;
            b_n <= b_n + bn; b_p <= b_p + bp;
            if (wcnt == 17'd65535) begin window <= 1'b0; settle <= 1; end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;
            if (&settle) begin settle <= 0; report <= 1'b1; end
        end
        if (report) begin
            out <= {b_p, b_n, a_p, a_n}; nout <= 16;
        end else if (tx_taken && !(rx_valid && (have_op || have_r))) begin
            out <= {8'h00, out[127:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {1'b0, x};
endmodule
