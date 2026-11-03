// RAM32X1S and RAM32X1D (hand-instantiated; yosys itself uses RAM32M), 8
// lanes each, with initial contents, over the Mojo's serial port:
//   'W' a d -> 'K'     wa = a; ms[a % 32] = md[a % 32] = d
//   'R' a   -> 3 bytes: ms[a % 32], md[wa % 32] (SPO), md[a % 32] (DPO)
// Lane i's INIT is 32'h9E3779B9 rotated left by 4i (ms) and its complement
// (md). Checked by tools/ram32-test.py.
module ram32 (
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

    reg  [4:0] wa = 0, ra = 0;
    reg  [7:0] d = 0;
    reg        we = 0;
    wire [7:0] qs, spo, dpo;
    localparam [31:0] SEED = 32'h9E3779B9;
    genvar i;
    generate for (i = 0; i < 8; i = i + 1) begin : lane
        localparam [31:0] INIT = (SEED << (4 * i)) | (SEED >> (32 - 4 * i));
        RAM32X1S #(.INIT(INIT)) s (.WCLK(clk), .WE(we), .D(d[i]), .O(qs[i]),
            .A0(ra[0]), .A1(ra[1]), .A2(ra[2]), .A3(ra[3]), .A4(ra[4]));
        RAM32X1D #(.INIT(~INIT)) dp (.WCLK(clk), .WE(we), .D(d[i]), .SPO(spo[i]), .DPO(dpo[i]),
            .A0(wa[0]), .A1(wa[1]), .A2(wa[2]), .A3(wa[3]), .A4(wa[4]),
            .DPRA0(ra[0]), .DPRA1(ra[1]), .DPRA2(ra[2]), .DPRA3(ra[3]), .DPRA4(ra[4]));
    end endgenerate

    reg [7:0]  op = 0, arg0 = 0;
    reg [1:0]  nargs = 0;
    reg        reply = 0;
    reg [23:0] out = 0;
    reg [1:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        we <= 1'b0;
        reply <= 1'b0;
        if (rx_valid) begin
            if (nargs == 0) begin
                // Unknown bytes are ignored (stray AVR bytes at startup).
                if (rx_data == "W" || rx_data == "R") begin op <= rx_data; nargs <= 1; end
            end else if (nargs == 1) begin
                if (op == "R") begin ra <= rx_data[4:0]; nargs <= 0; reply <= 1'b1; end
                else begin arg0 <= rx_data; nargs <= 2; end
            end else begin
                // The single-port RAM writes at its read address, ra.
                wa <= arg0[4:0]; ra <= arg0[4:0]; d <= rx_data; we <= 1'b1; nargs <= 0;
                out <= "K"; nout <= 1;
            end
        end
        if (reply) begin
            out <= {dpo, spo, qs}; nout <= 3;
        end else if (tx_taken) begin
            out <= {8'h00, out[23:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = qs;
endmodule
