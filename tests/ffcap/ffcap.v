// Flip-flop capacity: 8 bit-lanes of a DEPTH-stage shift register with a
// synchronous reset (so yosys can't make SRLs of it), 7680 FFs: more than the
// xc6slx9's 5720 main slice FFs, so it only fits with FFs on 5FFs through
// pass-through LUT5s. Over the Mojo's serial port:
//   'R' x -> 'K'      reset every stage to 0
//   'S' d -> 2 bytes   shift d in, reply {stage DEPTH-1, stage DEPTH/2-1} as
//                      they were before this shift.
// Checked by tools/ffcap-test.py.
module ffcap (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    localparam DEPTH = 960;
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [7:0] d = 0;
    reg       ce = 0, clr = 0;
    reg [7:0] sr [0:DEPTH-1];
    integer k;
    always @(posedge clk)
        if (clr)
            for (k = 0; k < DEPTH; k = k + 1) sr[k] <= 8'h00;
        else if (ce) begin
            sr[0] <= d;
            for (k = 1; k < DEPTH; k = k + 1) sr[k] <= sr[k-1];
        end

    reg        have_op = 0;
    reg [7:0]  op = 0;
    reg [15:0] out = 0;
    reg [1:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        ce <= 1'b0;
        clr <= 1'b0;
        if (rx_valid) begin
            // Unknown bytes are ignored (stray AVR bytes at startup).
            if (!have_op) begin
                if (rx_data == "R" || rx_data == "S") begin op <= rx_data; have_op <= 1'b1; end
            end else begin
                have_op <= 1'b0;
                if (op == "R") begin
                    clr <= 1'b1; out <= "K"; nout <= 1;
                end else begin
                    out <= {sr[DEPTH/2-1], sr[DEPTH-1]}; nout <= 2; d <= rx_data; ce <= 1'b1;
                end
            end
        end else if (tx_taken) begin
            out <= {8'h00, out[15:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = sr[DEPTH-1];
endmodule
