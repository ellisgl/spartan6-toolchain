// DCM fixed phase shift, checked on the board. CLK0 (fed back, CLK_FEEDBACK =
// 1X) is shifted by PHASE_SHIFT / 256 of the 20 ns period (tools/phase-test.py
// rewrites PHASE_SHIFT in the routed design). The shifted clock is also
// sampled as data on the unshifted 50 MHz clock: shifted later (+90 deg) it is
// still low when the board clock rises, shifted earlier (-90 deg) already
// high.
// 'P' samples it for 65536 cycles and replies with the count of 1s (4 bytes,
// LE) and {7'b0, locked}.
module phase (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire c0, fb, locked;
    DCM_SP #(
        .CLKIN_PERIOD(20.0),
        .CLK_FEEDBACK("1X"),
        .CLKOUT_PHASE_SHIFT("FIXED"),
        .PHASE_SHIFT(64),        // rewritten by tools/phase-test.py
        .STARTUP_WAIT("FALSE")
    ) dcm (
        .CLKIN(clk), .CLKFB(fb), .RST(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), .PSCLK(1'b0),
        .DSSEN(1'b0), .CLK0(c0), .LOCKED(locked)
    );
    BUFG bufg_0 (.I(c0), .O(fb));
    // Something clocked by the shifted clock, so it's a real clock net.
    reg t = 0;
    always @(posedge fb) t <= ~t;

    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg        s = 0;
    always @(posedge clk) s <= fb;   // the shifted clock, as data
    reg        run = 0, report = 0;
    reg [15:0] n = 0;
    reg [31:0] ones = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid && rx_data == "P" && !run) begin
            run <= 1'b1; n <= 0; ones <= 0;
        end else if (run) begin
            n <= n + 1'b1;
            ones <= ones + s;
            if (&n) begin run <= 1'b0; report <= 1'b1; end
        end
    end

    reg [39:0] out = 0;
    reg [2:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            out <= {7'b0, locked, ones}; nout <= 5;
        end else if (tx_taken) begin
            out <= {8'h00, out[39:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {t, 6'b0, locked};
endmodule
