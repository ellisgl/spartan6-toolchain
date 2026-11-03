// Differential I/O (LVDS_33): an OBUFDS driven from a register, an IBUFDS
// read into one, on two header pin pairs (see pins.ucf; leave them
// unconnected, or loop the output pair onto the input pair). The input uses
// the internal 100 ohm termination (DIFF_TERM), which an LVDS driver needs.
//   'S' x -> 'K'  drive x[0] on the output pair
//   'R'   -> 1 byte: {6'b0, output register, input pair as read}
// Checked by tools/lvds-test.py.
module lvds (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire       out_p,
    output wire       out_n,
    input  wire       in_p,
    input  wire       in_n,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg o = 0;
    OBUFDS #(.IOSTANDARD("LVDS_33")) obd (.I(o), .O(out_p), .OB(out_n));
    wire i;
    IBUFDS #(.IOSTANDARD("LVDS_33"), .DIFF_TERM("TRUE")) ibd (.I(in_p), .IB(in_n), .O(i));
    reg r = 0;
    always @(posedge clk) r <= i;

    reg       have_op = 0;
    reg [7:0] out = 0;
    reg       nout = 0;
    wire      tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout && ready),
                                  .data(out), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (rx_valid) begin
            // Unknown bytes are ignored (stray AVR bytes at startup).
            if (have_op) begin
                have_op <= 1'b0;
                o <= rx_data[0];
                out <= "K"; nout <= 1'b1;
            end else if (rx_data == "S") begin
                have_op <= 1'b1;
            end else if (rx_data == "R") begin
                out <= {6'b0, o, r}; nout <= 1'b1;
            end
        end else if (tx_taken) begin
            nout <= 1'b0;
        end
    end
    assign led = {6'b0, o, r};
endmodule
