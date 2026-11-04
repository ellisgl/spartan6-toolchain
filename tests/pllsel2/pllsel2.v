// As pll2, on a PLL_ADV with its clock on CLKIN2 and CLKINSEL tied to 0
// (CLKIN1 unused, tied low). From the 50 MHz board clock: VCO 500 MHz, CLKOUT0 / 4 =
// 125 MHz, CLKOUT1 / 7 = 71.43 MHz, CLKOUT2 / 10 = 50 MHz.
// 'C' counts each for 65536 cycles of the 50 MHz clock; the reply is 3 x 4
// bytes (CLKOUT0, CLKOUT1, CLKOUT2; little-endian) and 1 byte {7'b0, locked}.
// Checked by tools/pll2-test.py (same replies).
module pllsel2 (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire fb, o0, o1, o2, locked;
    PLL_ADV #(
        .CLKIN1_PERIOD(20.0),
        .CLKIN2_PERIOD(20.0),
        .CLKFBOUT_MULT(10),
        .CLKOUT0_DIVIDE(4),
        .CLKOUT1_DIVIDE(7),
        .CLKOUT2_DIVIDE(10)
    ) pll (
        .CLKIN1(1'b0), .CLKIN2(clk), .CLKINSEL(1'b0), .CLKFBIN(fb), .RST(1'b0),
        .DCLK(1'b0), .DEN(1'b0), .DWE(1'b0), .DADDR(5'd0), .DI(16'd0), .REL(1'b0),
        .CLKFBOUT(fb), .CLKOUT0(o0), .CLKOUT1(o1), .CLKOUT2(o2), .LOCKED(locked)
    );
    wire g0, g1, g2;
    BUFG bufg_2  (.I(o2),  .O(g2));
    BUFG bufg_0  (.I(o0),  .O(g0));
    BUFG bufg_1  (.I(o1),  .O(g1));

    // --- measurement ---
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [16:0] wcnt = 0;
    reg        window = 0;
    reg [9:0]  settle = 0;
    reg        report = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid && rx_data == "C" && !window && settle == 0) begin
            window <= 1'b1;
            wcnt   <= 0;
        end else if (window) begin
            wcnt <= wcnt + 1'b1;
            if (wcnt == 17'd65535) begin
                window <= 1'b0;
                settle <= 1;
            end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;  // let the counters stop and settle
            if (&settle) begin
                settle <= 0;
                report <= 1'b1;
            end
        end
    end

    reg [31:0] n_p0 = 0;
    reg [2:0]  w_p0 = 0;
    always @(posedge g0) begin
        w_p0 <= {w_p0[1:0], window};
        if (w_p0[2:1] == 2'b01) n_p0 <= 0;
        else if (w_p0[1]) n_p0 <= n_p0 + 1'b1;
    end
    reg [31:0] n_p1 = 0;
    reg [2:0]  w_p1 = 0;
    always @(posedge g1) begin
        w_p1 <= {w_p1[1:0], window};
        if (w_p1[2:1] == 2'b01) n_p1 <= 0;
        else if (w_p1[1]) n_p1 <= n_p1 + 1'b1;
    end
    reg [31:0] n_p2 = 0;
    reg [2:0]  w_p2 = 0;
    always @(posedge g2) begin
        w_p2 <= {w_p2[1:0], window};
        if (w_p2[2:1] == 2'b01) n_p2 <= 0;
        else if (w_p2[1]) n_p2 <= n_p2 + 1'b1;
    end

    reg [103:0] out = 0;
    reg [3:0]   nout = 0;
    wire        tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            out  <= {7'b0, locked, n_p2, n_p1, n_p0};
            nout <= 13;
        end else if (tx_taken) begin
            out  <= {8'h00, out[103:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = {7'b0, locked};
endmodule
