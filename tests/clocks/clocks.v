// Clock management test (DCM_SP, PLL_BASE) over the Mojo's USB serial port.
// From the 50 MHz board clock:
//   fx:   DCM_SP CLKFX = 50 * 7 / 5 = 70 MHz
//   p0:   PLL_BASE VCO 400 MHz / 5  = 80 MHz
//   p1:   PLL_BASE VCO 400 MHz / 12 = 33.33 MHz
// 'C' starts a measurement: for 65536 cycles of the 50 MHz clock each
// generated clock's counter runs; the reply is 3 x 4 bytes (little-endian
// counts, expected 65536 * f / 50 MHz, within a few counts) plus 1 byte of
// lock status {6'b0, pll_locked, dcm_locked}. Checked by
// tools/clocks-test.py.
module clocks (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    // --- clock generation ---
    wire clk_fb_dcm, clkfx_u, dcm_locked;
    DCM_SP #(
        .CLKIN_PERIOD(20.0),
        .CLKFX_MULTIPLY(7),
        .CLKFX_DIVIDE(5),
        .CLK_FEEDBACK("NONE"),
        .STARTUP_WAIT("FALSE")
    ) dcm (
        .CLKIN(clk), .CLKFB(1'b0), .RST(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), .PSCLK(1'b0),
        .DSSEN(1'b0), .CLKFX(clkfx_u), .LOCKED(dcm_locked)
    );
    wire clk_fx;
    BUFG bufg_fx (.I(clkfx_u), .O(clk_fx));

    wire pll_fb, p0_u, p1_u, pll_locked;
    PLL_BASE #(
        .CLKIN_PERIOD(20.0),
        .DIVCLK_DIVIDE(1),
        .CLKFBOUT_MULT(8),
        .CLKOUT0_DIVIDE(5),
        .CLKOUT1_DIVIDE(12),
        .COMPENSATION("INTERNAL")
    ) pll (
        .CLKIN(clk), .CLKFBIN(pll_fb), .RST(1'b0),
        .CLKFBOUT(pll_fb), .CLKOUT0(p0_u), .CLKOUT1(p1_u), .LOCKED(pll_locked)
    );
    wire clk_p0, clk_p1;
    BUFG bufg_p0 (.I(p0_u), .O(clk_p0));
    BUFG bufg_p1 (.I(p1_u), .O(clk_p1));

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

    // One counter per generated clock, enabled by the synchronized window
    // and cleared when a new window starts.
    reg [31:0] c_fx = 0, c_p0 = 0, c_p1 = 0;
    reg [2:0]  w_fx = 0, w_p0 = 0, w_p1 = 0;
    always @(posedge clk_fx) begin
        w_fx <= {w_fx[1:0], window};
        if (w_fx[2:1] == 2'b01) c_fx <= 0;
        else if (w_fx[1]) c_fx <= c_fx + 1'b1;
    end
    always @(posedge clk_p0) begin
        w_p0 <= {w_p0[1:0], window};
        if (w_p0[2:1] == 2'b01) c_p0 <= 0;
        else if (w_p0[1]) c_p0 <= c_p0 + 1'b1;
    end
    always @(posedge clk_p1) begin
        w_p1 <= {w_p1[1:0], window};
        if (w_p1[2:1] == 2'b01) c_p1 <= 0;
        else if (w_p1[1]) c_p1 <= c_p1 + 1'b1;
    end

    reg [103:0] out = 0;
    reg [3:0]   nout = 0;
    wire        tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            // The counters are stopped and stable by now.
            out  <= {6'b0, pll_locked, dcm_locked, c_p1, c_p0, c_fx};
            nout <= 13;
        end else if (tx_taken) begin
            out  <= {8'h00, out[103:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = {6'b0, pll_locked, dcm_locked};
endmodule
