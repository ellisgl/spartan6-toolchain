// On-chip speed test. A PLL makes the test clock (from the 50 MHz board
// clock); tools/speed-test.py rewrites the PLL's multiplier and divider in
// the routed design for each frequency, so placement and routing stay the
// same while the clock sweeps.
//
// The path under test: three chained 32-bit additions and an XOR, from the
// a/b/c/d registers to `fast`. Inputs come from an LFSR and change every 4
// test clocks; `fast` captures the result 1 clock after the change (the
// timed path) and `slow` 3 clocks after (always settled). Every mismatch is
// counted. Static timing sees both captures as the same one-cycle path.
//
// 'T' over the serial port runs the test for 2^22 board clocks (~84 ms) and
// replies with 9 bytes: test-clock cycles counted in that window (4, LE),
// error count (4, LE), PLL LOCKED (1).
module speed (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    // --- test clock ---
    wire pll_fb, tclk_u, locked;
    PLL_BASE #(
        .CLKIN_PERIOD(20.0),
        .DIVCLK_DIVIDE(1),
        .CLKFBOUT_MULT(8),     // rewritten by tools/speed-test.py
        .CLKOUT0_DIVIDE(4),    // rewritten by tools/speed-test.py
        .COMPENSATION("INTERNAL")
    ) pll (
        .CLKIN(clk), .CLKFBIN(pll_fb), .RST(1'b0),
        .CLKFBOUT(pll_fb), .CLKOUT0(tclk_u), .LOCKED(locked)
    );
    wire tclk;
    BUFG bufg_t (.I(tclk_u), .O(tclk));

    // --- control, 50 MHz domain ---
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg [22:0] wcnt = 0;
    reg        run = 0;
    reg [9:0]  settle = 0;
    reg        report = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid && rx_data == "T" && !run && settle == 0) begin
            run  <= 1'b1;
            wcnt <= 0;
        end else if (run) begin
            wcnt <= wcnt + 1'b1;
            if (&wcnt[21:0]) begin
                run    <= 1'b0;
                settle <= 1;
            end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;
            if (&settle) begin
                settle <= 0;
                report <= 1'b1;
            end
        end
    end

    // --- test, tclk domain ---
    reg [2:0] run_s = 0;
    always @(posedge tclk) run_s <= {run_s[1:0], run};
    wire t_run = run_s[1];
    wire t_start = run_s[2:1] == 2'b01;

    reg [127:0] lfsr = 128'h0123_4567_89ab_cdef_fedc_ba98_7654_3210;
    reg [1:0]   phase = 0;
    reg [31:0]  a = 0, b = 0, c = 0, d = 0;
    wire [31:0] f = ((a + b) + c) + d ^ {a[15:0], b[31:16]};
    reg [31:0]  fast = 0, slow = 0;
    reg         check = 0;
    reg [31:0]  errors = 0, cycles = 0;
    always @(posedge tclk) begin
        phase <= phase + 1'b1;
        case (phase)
            2'd0: begin
                // Launch new inputs.
                lfsr <= {lfsr[126:0], lfsr[127] ^ lfsr[125] ^ lfsr[100] ^ lfsr[98]};
                {a, b, c, d} <= lfsr;
                check <= 1'b0;
            end
            2'd1: fast <= f;   // one cycle after launch: the timed capture
            2'd3: begin
                slow  <= f;    // three cycles after: settled
                check <= 1'b1;
            end
            default: ;
        endcase
        if (t_start) begin
            errors <= 0;
            cycles <= 0;
        end else if (t_run) begin
            cycles <= cycles + 1'b1;
            if (check && phase == 2'd0 && fast != slow)
                errors <= errors + 1'b1;
        end
    end

    // --- report, 50 MHz domain (the counters are stopped by then) ---
    reg [71:0] out = 0;
    reg [3:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            out  <= {7'b0, locked, errors, cycles};
            nout <= 9;
        end else if (tx_taken) begin
            out  <= {8'h00, out[71:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = {locked, errors[6:0]};
endmodule
