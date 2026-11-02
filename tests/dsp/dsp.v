// DSP48A1 test over the Mojo's USB serial port (little-endian):
//   'M' a(3) b(3) -> 5 bytes: $signed(a[17:0]) * $signed(b[17:0])   (36 bits)
//   'A' a(3) b(3) -> 6 bytes: acc += a * b, then acc                (48 bits)
//   'Z'           -> 'K', acc = 0
// Checked by tools/dsp-test.py.
module dsp (
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

    reg [55:0] cmd = 0;   // op, a (3), b (3)
    reg [2:0]  ncmd = 0;
    wire [7:0] op = cmd[7:0];
    wire signed [17:0] a = cmd[25:8];
    wire signed [17:0] b = cmd[49:32];

    // Registered operands into the multiplier, registered product.
    reg signed [17:0] ar = 0, br = 0;
    reg signed [35:0] prod = 0;
    reg signed [47:0] acc = 0;
    reg        acc_en = 0, acc_clr = 0;
    always @(posedge clk) begin
        ar   <= a;
        br   <= b;
        prod <= ar * br;
        if (acc_clr)
            acc <= 0;
        else if (acc_en)
            acc <= acc + ar * br;
    end

    reg [47:0] out = 0;
    reg [2:0]  nout = 0;
    reg [3:0]  phase = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    always @(posedge clk) begin
        acc_en  <= 1'b0;
        acc_clr <= 1'b0;
        if (rx_valid && phase == 0) begin
            if (ncmd == 0)
                cmd <= {48'h0, rx_data};
            else
                cmd[ncmd * 8 +: 8] <= rx_data;
            ncmd <= ncmd + 1'b1;
        end
        if (phase == 0 && ncmd != 0) begin
            if (op == "Z") begin
                acc_clr <= 1'b1;
                out   <= "K";
                nout  <= 1;
                ncmd  <= 0;
            end else if (ncmd == 7 && (op == "M" || op == "A")) begin
                phase <= 1;
                ncmd  <= 0;
            end else if (op != "M" && op != "A") begin
                ncmd <= 0;
            end
        end
        case (phase)
            0: ;
            // 1: operands registered; 2: product (and acc) registered
            1: begin
                phase  <= 2;
                acc_en <= op == "A";
            end
            2: phase <= 3;
            3: begin
                if (op == "M") begin
                    out  <= {{12{prod[35]}}, prod};
                    nout <= 5;
                end else begin
                    out  <= acc;
                    nout <= 6;
                end
                phase <= 0;
            end
            default: phase <= 0;
        endcase
        if (tx_taken) begin
            out  <= {8'h00, out[47:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = acc[7:0];
endmodule
