// Latches (slice FFs in latch mode) over the Mojo's serial port:
//   'D' x -> 'K'  data = x
//   'G' x -> 'K'  gate = x[0]
//   'C' x -> 'K'  clr = x[0] (clear for qa, preset for qb)
//   'Q'   -> 3 bytes: qa, qb, qn
// qa: 8 latches, transparent while gate is 1, cleared by clr (LDCE).
// qb: 8 latches, transparent while gate is 1, preset by clr (LDPE).
// qn: 8 latches, transparent while gate is 0 (inverted gate).
// Checked by tools/latch-test.py.
module latch (
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

    reg [7:0] data = 0;
    reg       gate = 0, clr = 0;

    reg [7:0] qa, qb, qn;
    always @* if (clr) qa = 8'h00; else if (gate) qa = data;
    always @* if (clr) qb = 8'hFF; else if (gate) qb = data;
    always @* if (!gate) qn = data;

    reg [7:0]  op = 0;
    reg        have_op = 0;
    reg [23:0] out = 0;
    reg [1:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    // Two cycles between a write and a later 'Q' (the serial port is far
    // slower), so the latches have settled before they're sampled.
    reg [7:0] qa_s, qb_s, qn_s;
    always @(posedge clk) begin
        qa_s <= qa; qb_s <= qb; qn_s <= qn;
        if (rx_valid) begin
            if (!have_op) begin
                // Unknown bytes are ignored (the AVR can send a stray byte
                // at startup, which would otherwise shift every command).
                if (rx_data == "Q") begin
                    out <= {qn_s, qb_s, qa_s}; nout <= 3;
                end else if (rx_data == "D" || rx_data == "G" || rx_data == "C") begin
                    op <= rx_data; have_op <= 1'b1;
                end
            end else begin
                have_op <= 1'b0;
                case (op)
                    "D": data <= rx_data;
                    "G": gate <= rx_data[0];
                    "C": clr  <= rx_data[0];
                    default: ;
                endcase
                out <= "K"; nout <= 1;
            end
        end else if (tx_taken) begin
            out  <= {8'h00, out[23:8]};
            nout <= nout - 1'b1;
        end
    end
    assign led = qa;
endmodule
