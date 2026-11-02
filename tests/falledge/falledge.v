// Falling-edge flip-flops and shift registers (the slice's inverted clock,
// CLKINV). p toggles on every rising edge; between rising edges it holds.
//   n  (falling-edge FF)  samples p half a cycle after it changed, so at the
//      next rising edge n == p. A rising-edge FF would give n == ~p.
//   sr (falling-edge 20-deep shift register of p, an SRL): its last stage at
//      the next rising edge is p ^ 1 (an odd number of toggles back), where a
//      rising-edge one would give p.
//   ram (falling-edge 64x1 RAM): an LFSR bit d and address a change on rising
//      edges; the falling edge writes d at a, so reading a (asynchronously)
//      at the next rising edge gives d. A rising-edge write wouldn't be there
//      yet, and the read would see the bit from 64 cycles earlier.
// 'T' runs 2^22 cycles of checks and replies with 12 bytes: mismatch counts
// for n, sr and ram (4 each, LE). Checked by tools/falledge-test.py.
module falledge (
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

    reg p = 0;
    always @(posedge clk) p <= ~p;

    reg n = 0;
    always @(negedge clk) n <= p;

    reg [19:0] sr = 0;
    always @(negedge clk) sr <= {sr[18:0], p};

    reg [5:0]  a = 0;
    reg [15:0] lfsr = 16'hACE1;
    always @(posedge clk) begin
        a    <= a + 1'b1;
        lfsr <= {lfsr[14:0], lfsr[15] ^ lfsr[13] ^ lfsr[12] ^ lfsr[10]};
    end
    wire d = lfsr[0];
    reg mem [0:63];
    integer k;
    initial for (k = 0; k < 64; k = k + 1) mem[k] = 0;
    always @(negedge clk) mem[a] <= d;
    wire ram_q = mem[a];

    reg        run = 0;
    reg [22:0] cnt = 0;
    reg [1:0]  warm = 0;
    reg [31:0] e_n = 0, e_sr = 0, e_ram = 0;
    reg        report = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        if (rx_valid && rx_data == "T" && !run) begin
            run <= 1'b1; cnt <= 0; warm <= 0;
            e_n <= 0; e_sr <= 0; e_ram <= 0;
        end else if (run) begin
            cnt <= cnt + 1'b1;
            if (warm != 2'b11) warm <= warm + 1'b1;
            else begin
                if (n != p) e_n <= e_n + 1'b1;
                if (sr[19] != ~p) e_sr <= e_sr + 1'b1;
                if (ram_q != d) e_ram <= e_ram + 1'b1;
            end
            if (&cnt[21:0]) begin run <= 1'b0; report <= 1'b1; end
        end
    end

    reg [95:0] out = 0;
    reg [3:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        if (report) begin
            out <= {e_ram, e_sr, e_n}; nout <= 12;
        end else if (tx_taken) begin
            out <= {8'h00, out[95:8]}; nout <= nout - 1'b1;
        end
    end
    assign led = {p, n, sr[19], ram_q, e_n[3:0]};
endmodule
