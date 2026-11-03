// IDDR2 with its synchronous reset (R) driven from logic, on the board; as
// tests/iddr, plus x[2] of 'S' holds R: Q0/Q1 then read 0 whatever the pin.
// Checked by tools/iddrsr-test.py.
// IDDR2 (DDR input) on the board. Header pin P51 is driven by a register in
// the 50 MHz domain (bidirectional pad, output always enabled) and read back
// through the same pad by an IDDR2 (C0 = clk, C1 = ~clk).
//   'S' x -> 'K'  x = 0 / 1: hold the pin low / high; 2: toggle it on every
//                 rising clock edge
//   'C'   -> 12 bytes (LE), over 65536 cycles of the clock: how often Q0
//                 (rising-edge sample) read 1, how often Q1 (falling-edge
//                 sample) read 1, and how often Q0 != Q1. Toggling, the pin
//                 changes a few ns after each rising edge: the falling-edge
//                 sample sees the new value and the rising-edge one the old,
//                 so Q0 != Q1 every cycle.
// Checked by tools/iddr-test.py.
module iddrsr (
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

    reg [1:0] mode = 2;
    reg       rst = 0;
    reg       drive = 0;
    always @(posedge clk) drive <= mode[1] ? ~drive : mode[0];
    wire back, q0, q1;
    IOBUF iob (.IO(pad), .I(drive), .T(1'b0), .O(back));
    IDDR2 #(.DDR_ALIGNMENT("NONE"), .INIT_Q0(1'b0), .INIT_Q1(1'b0), .SRTYPE("SYNC")) id (
        .Q0(q0), .Q1(q1), .C0(clk), .C1(~clk), .CE(1'b1), .D(back), .R(rst), .S(1'b0));
    reg r0 = 0, r1 = 0;
    always @(posedge clk) begin r0 <= q0; r1 <= q1; end

    reg [16:0] wcnt = 0;
    reg        window = 0, report = 0;
    reg [9:0]  settle = 0;
    reg [31:0] n0 = 0, n1 = 0, nd = 0;
    reg        have_op = 0;
    reg [95:0] out = 0;
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
                mode <= rx_data[1:0];
                rst  <= rx_data[2];
                out <= "K"; nout <= 1;
            end else if (rx_data == "S") begin
                have_op <= 1'b1;
            end else if (rx_data == "C" && !window && settle == 0) begin
                window <= 1'b1; wcnt <= 0; n0 <= 0; n1 <= 0; nd <= 0;
            end
        end
        if (window) begin
            wcnt <= wcnt + 1'b1;
            n0 <= n0 + r0;
            n1 <= n1 + r1;
            nd <= nd + (r0 ^ r1);
            if (wcnt == 17'd65535) begin window <= 1'b0; settle <= 1; end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;
            if (&settle) begin settle <= 0; report <= 1'b1; end
        end
        if (report) begin
            out <= {nd, n1, n0}; nout <= 12;
        end else if (tx_taken && !(rx_valid && have_op)) begin
            out <= {8'h00, out[95:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {5'b0, rst, mode};
endmodule
