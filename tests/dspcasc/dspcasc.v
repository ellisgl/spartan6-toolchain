// DSP48A1 cascades on the board: three DSPs chained through PCOUT -> PCIN,
// BCOUT -> B (the dedicated BCIN) and CARRYOUT -> CARRYIN.
//   d0: P0 = A0 * B                        (OPMODE X = M)
//   d1: P1 = PCIN + A1 * B', B' = d0 BCOUT  (OPMODE X = M, Z = PCIN)
//   d2: P2 = PCIN + A2 * B'' + CARRYIN       (CARRYIN = d1's CARRYOUT)
// 18 x 18 signed products, 48-bit sums; A and B registered once (A1REG,
// B1REG), P registered (PREG). Inputs are held, so the pipeline settles.
//   'L' + 12 bytes -> 'K'  a0, a1, a2, b (each 3 bytes LE, low 18 bits)
//   'R'             -> 7 bytes: P2 (6 bytes LE), then {7'b0, d1 CARRYOUTF}
//                     (CARRYOUT itself only reaches the next DSP)
// Checked by tools/dspcasc-test.py.
module dspcasc (
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

    reg [95:0] in = 0;
    wire [17:0] a0 = in[17:0], a1 = in[41:24], a2 = in[65:48], b = in[89:72];
    wire [47:0] pc0, pc1, p2;
    wire [17:0] bc0, bc1;
    wire        co1, co1f;
    DSP48A1 #(.A0REG(0), .A1REG(1), .B0REG(0), .B1REG(1), .CREG(0), .DREG(0), .MREG(0), .PREG(1),
              .OPMODEREG(0), .CARRYINREG(0), .CARRYOUTREG(0), .CARRYINSEL("OPMODE5")) d0 (
        .CLK(clk), .A(a0), .B(b), .C(48'd0), .D(18'd0), .OPMODE(8'b00000001), .CARRYIN(1'b0),
        .CEA(1'b1), .CEB(1'b1), .CEC(1'b0), .CED(1'b0), .CEM(1'b0), .CEP(1'b1), .CEOPMODE(1'b0), .CECARRYIN(1'b0),
        .RSTA(1'b0), .RSTB(1'b0), .RSTC(1'b0), .RSTD(1'b0), .RSTM(1'b0), .RSTP(1'b0), .RSTOPMODE(1'b0), .RSTCARRYIN(1'b0),
        .PCIN(48'd0), .PCOUT(pc0), .BCOUT(bc0), .CARRYOUT(), .P(), .M(), .CARRYOUTF());
    DSP48A1 #(.A0REG(0), .A1REG(1), .B0REG(0), .B1REG(1), .CREG(0), .DREG(0), .MREG(0), .PREG(1),
              .OPMODEREG(0), .CARRYINREG(0), .CARRYOUTREG(0), .CARRYINSEL("OPMODE5")) d1 (
        .CLK(clk), .A(a1), .B(bc0), .C(48'd0), .D(18'd0), .OPMODE(8'b00000101), .CARRYIN(1'b0),
        .CEA(1'b1), .CEB(1'b1), .CEC(1'b0), .CED(1'b0), .CEM(1'b0), .CEP(1'b1), .CEOPMODE(1'b0), .CECARRYIN(1'b0),
        .RSTA(1'b0), .RSTB(1'b0), .RSTC(1'b0), .RSTD(1'b0), .RSTM(1'b0), .RSTP(1'b0), .RSTOPMODE(1'b0), .RSTCARRYIN(1'b0),
        .PCIN(pc0), .PCOUT(pc1), .BCOUT(bc1), .CARRYOUT(co1), .P(), .M(), .CARRYOUTF(co1f));
    DSP48A1 #(.A0REG(0), .A1REG(1), .B0REG(0), .B1REG(1), .CREG(0), .DREG(0), .MREG(0), .PREG(1),
              .OPMODEREG(0), .CARRYINREG(0), .CARRYOUTREG(0), .CARRYINSEL("CARRYIN")) d2 (
        .CLK(clk), .A(a2), .B(bc1), .C(48'd0), .D(18'd0), .OPMODE(8'b00000101), .CARRYIN(co1),
        .CEA(1'b1), .CEB(1'b1), .CEC(1'b0), .CED(1'b0), .CEM(1'b0), .CEP(1'b1), .CEOPMODE(1'b0), .CECARRYIN(1'b0),
        .RSTA(1'b0), .RSTB(1'b0), .RSTC(1'b0), .RSTD(1'b0), .RSTM(1'b0), .RSTP(1'b0), .RSTOPMODE(1'b0), .RSTCARRYIN(1'b0),
        .PCIN(pc1), .P(p2), .PCOUT(), .BCOUT(), .CARRYOUT(), .M(), .CARRYOUTF());

    reg [3:0]  nload = 0;
    reg        loading = 0;
    reg [55:0] out = 0;
    reg [2:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (rx_valid) begin
            if (loading) begin
                in <= {rx_data, in[95:8]};
                nload <= nload + 1'b1;
                if (nload == 4'd11) begin loading <= 1'b0; out <= "K"; nout <= 1; end
            end else if (rx_data == "L") begin
                // Unknown bytes are ignored (stray AVR bytes at startup).
                loading <= 1'b1; nload <= 0;
            end else if (rx_data == "R") begin
                out <= {7'b0, co1f, p2}; nout <= 7;
            end
        end else if (tx_taken) begin
            out <= {8'h00, out[55:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = p2[7:0];
endmodule
