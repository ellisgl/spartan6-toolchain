// DCM_SP in DLL mode with CLK2X fed back through a BUFG (CLK_FEEDBACK = 2X).
// From the 50 MHz board clock:
//   CLK0  50 MHz, CLK90 50 MHz, CLK2X 100 MHz, CLKDV 50 / 2.5 = 20 MHz,
//   CLKFX 50 * 3 / 2 = 75 MHz.
// 'C' counts each clock for 65536 cycles of the 50 MHz clock; the reply is
// 5 x 4 bytes (CLK0, CLK90, CLK2X, CLKDV, CLKFX; little-endian) and 1 byte
// {7'b0, locked}. Checked by tools/dll-test.py --design dll2x.
module dll2x (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire c0, c90, c2x, cdv, cfx, g0, locked;
    DCM_SP #(
        .CLKIN_PERIOD(20.0),
        .CLK_FEEDBACK("2X"),
        .CLKDV_DIVIDE(2.5),
        .CLKFX_MULTIPLY(3),
        .CLKFX_DIVIDE(2),
        .STARTUP_WAIT("FALSE")
    ) dcm (
        .CLKIN(clk), .CLKFB(g2x), .RST(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), .PSCLK(1'b0),
        .DSSEN(1'b0),
        .CLK0(c0), .CLK90(c90), .CLK2X(c2x), .CLKDV(cdv), .CLKFX(cfx), .LOCKED(locked)
    );
    wire g90, g2x, gdv, gfx;
    BUFG bufg_0  (.I(c0),  .O(g0));
    BUFG bufg_90 (.I(c90), .O(g90));
    BUFG bufg_2x (.I(c2x), .O(g2x));
    BUFG bufg_dv (.I(cdv), .O(gdv));
    BUFG bufg_fx (.I(cfx), .O(gfx));

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

    reg [31:0] n_c0 = 0;
    reg [2:0]  w_c0 = 0;
    always @(posedge g0) begin
        w_c0 <= {w_c0[1:0], window};
        if (w_c0[2:1] == 2'b01) n_c0 <= 0;
        else if (w_c0[1]) n_c0 <= n_c0 + 1'b1;
    end
    reg [31:0] n_c90 = 0;
    reg [2:0]  w_c90 = 0;
    always @(posedge g90) begin
        w_c90 <= {w_c90[1:0], window};
        if (w_c90[2:1] == 2'b01) n_c90 <= 0;
        else if (w_c90[1]) n_c90 <= n_c90 + 1'b1;
    end
    reg [31:0] n_c2x = 0;
    reg [2:0]  w_c2x = 0;
    always @(posedge g2x) begin
        w_c2x <= {w_c2x[1:0], window};
        if (w_c2x[2:1] == 2'b01) n_c2x <= 0;
        else if (w_c2x[1]) n_c2x <= n_c2x + 1'b1;
    end
    reg [31:0] n_cdv = 0;
    reg [2:0]  w_cdv = 0;
    always @(posedge gdv) begin
        w_cdv <= {w_cdv[1:0], window};
        if (w_cdv[2:1] == 2'b01) n_cdv <= 0;
        else if (w_cdv[1]) n_cdv <= n_cdv + 1'b1;
    end
    reg [31:0] n_cfx = 0;
    reg [2:0]  w_cfx = 0;
    always @(posedge gfx) begin
        w_cfx <= {w_cfx[1:0], window};
        if (w_cfx[2:1] == 2'b01) n_cfx <= 0;
        else if (w_cfx[1]) n_cfx <= n_cfx + 1'b1;
    end

    reg [167:0] out = 0;
    reg [4:0]   nout = 0;
    wire        tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            out  <= {7'b0, locked, n_cfx, n_cdv, n_c2x, n_c90, n_c0};
            nout <= 21;
        end else if (tx_taken) begin
            out  <= {8'h00, out[167:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = {7'b0, locked};
endmodule
