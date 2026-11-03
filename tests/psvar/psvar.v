// DCM variable phase shift (CLKOUT_PHASE_SHIFT = VARIABLE), checked on
// the board. CLK0 (fed back, CLK_FEEDBACK = 1X) is shifted one step at a time
// through PSEN / PSINCDEC, clocked by the board clock on PSCLK; PSDONE ends
// each step. As in tests/phase, the shifted clock is sampled as data on the
// board clock: later reads low, earlier high.
//   '+' / '-' -> one step later / earlier, then 'D' once PSDONE comes back
//                ('T' if it doesn't within 1024 cycles)
//   'P'       -> sample 65536 cycles: count of 1s (4 bytes, LE), {7'b0, locked}
// Checked by tools/psvar-test.py.
module psvar (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire c0, fb, locked, psdone;
    reg  psen = 0, psincdec = 0;
    DCM_SP #(
        .CLKIN_PERIOD(20.0),
        .CLK_FEEDBACK("1X"),
        .CLKOUT_PHASE_SHIFT("VARIABLE"),
        .PHASE_SHIFT(0),
        .STARTUP_WAIT("FALSE")
    ) dcm (
        .CLKIN(clk), .CLKFB(fb), .RST(1'b0), .PSEN(psen), .PSINCDEC(psincdec), .PSCLK(clk),
        .PSDONE(psdone), .DSSEN(1'b0), .CLK0(c0), .LOCKED(locked)
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
    reg        stepping = 0, step_done = 0, step_timeout = 0;
    reg [9:0]  wait_n = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        psen <= 1'b0;
        step_done <= 1'b0;
        step_timeout <= 1'b0;
        if (rx_valid && rx_data == "P" && !run && !stepping) begin
            run <= 1'b1; n <= 0; ones <= 0;
        end else if (run) begin
            n <= n + 1'b1;
            ones <= ones + s;
            if (&n) begin run <= 1'b0; report <= 1'b1; end
        end
        if (rx_valid && (rx_data == "+" || rx_data == "-") && !run && !stepping) begin
            psen <= 1'b1;            // one PSCLK cycle
            psincdec <= rx_data == "+";
            stepping <= 1'b1;
            wait_n <= 0;
        end else if (stepping) begin
            wait_n <= wait_n + 1'b1;
            if (psdone) begin
                stepping <= 1'b0; step_done <= 1'b1;
            end else if (&wait_n) begin
                stepping <= 1'b0; step_timeout <= 1'b1;
            end
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
        end else if (step_done || step_timeout) begin
            out <= {32'b0, step_done ? "D" : "T"}; nout <= 1;
        end else if (tx_taken) begin
            out <= {8'h00, out[39:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {t, 6'b0, locked};
endmodule
