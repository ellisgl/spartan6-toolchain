// DSP48A1 pre-adder test over the Mojo's USB serial port (little-endian):
//   'P' d(3) b(3) a(3) -> 12 bytes: a * (d + b), then a * (d - b), each
//                         48 bits (the 18-bit sum wraps, as in the DSP)
// yosys packs each into one DSP48A1 with the pre-adder (D port, OPMODE[4],
// and OPMODE[6] for the subtraction) and the registers around it.
// Checked by tools/preadd-test.py.
module preadd (
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

    reg [79:0] cmd = 0;   // op, d (3), b (3), a (3)
    reg [3:0]  ncmd = 0;
    wire [7:0] op = cmd[7:0];

    // Registered operands, an 18-bit pre-add / pre-subtract, registered
    // products.
    reg signed [17:0] dr = 0, br = 0, ar = 0;
    wire signed [17:0] sum = dr + br, diff = dr - br;
    reg signed [47:0] p_add = 0, p_sub = 0;
    always @(posedge clk) begin
        dr    <= cmd[25:8];
        br    <= cmd[49:32];
        ar    <= cmd[73:56];
        p_add <= ar * sum;
        p_sub <= ar * diff;
    end

    reg [95:0] out = 0;
    reg [3:0]  nout = 0;
    reg [2:0]  phase = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    always @(posedge clk) begin
        if (rx_valid && phase == 0) begin
            if (ncmd == 0)
                cmd <= {72'h0, rx_data};
            else
                cmd[ncmd * 8 +: 8] <= rx_data;
            ncmd <= ncmd + 1'b1;
        end
        if (phase == 0 && ncmd != 0) begin
            if (op != "P")
                ncmd <= 0;   // stray byte (the AVR sends some at startup)
            else if (ncmd == 10) begin
                phase <= 1;
                ncmd  <= 0;
            end
        end
        case (phase)
            0: ;
            // 1: operands registered; 2: products registered; then a spare
            1, 2, 3: phase <= phase + 1'b1;
            4: begin
                out   <= {p_sub, p_add};
                nout  <= 12;
                phase <= 0;
            end
            default: phase <= 0;
        endcase
        if (tx_taken) begin
            out  <= {8'h00, out[95:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = p_add[7:0];
endmodule
